#!/usr/bin/env python3
"""Short actual-BGE resource/pause probe; only disposable synthetic sources.

Reuses bounded loopback/evidence helpers from benchmark-reader-service.py. CPU
percent is measured CPU-time delta / elapsed wall time, never ps's lifetime %CPU.
This does not measure model-actor admission duration or establish a hard CPU quota.
"""
import argparse
import contextlib
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
import tempfile
import time
import unittest

HELPER = Path(__file__).with_name('benchmark-reader-service.py')
spec = importlib.util.spec_from_file_location('reader_benchmark_helpers', HELPER)
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


def cpu_seconds(value):
    days, clock = value.strip().split('-', 1) if '-' in value else ('0', value.strip())
    parts = clock.split(':')
    if not 1 <= len(parts) <= 3:
        raise ValueError('invalid process CPU time')
    seconds = 0.
    for part in parts:
        seconds = seconds * 60 + float(part)
    return int(days) * 86400 + seconds


def process_threads(raw, pid):
    # Darwin ps -M emits its fixed USER/PID/thread columns despite -o pid=.
    count = sum(len(parts) >= 2 and parts[1] == str(pid) for parts in
                (line.split() for line in raw.splitlines()))
    return count or None


def sample(pid):
    raw = subprocess.check_output(['ps', '-p', str(pid), '-o', 'time=,rss='], text=True).strip().split()
    if len(raw) != 2:
        raise RuntimeError('synthetic service process disappeared')
    threads = subprocess.check_output(['ps', '-M', '-p', str(pid), '-o', 'pid='], text=True)
    return {'monotonic_seconds': time.monotonic(), 'cpu_seconds': cpu_seconds(raw[0]),
            'rss_bytes': int(raw[1]) * 1024,
            'process_threads': process_threads(threads, pid)}


def summarize(start, end):
    elapsed = end['monotonic_seconds'] - start['monotonic_seconds']
    cpu = end['cpu_seconds'] - start['cpu_seconds']
    if elapsed <= 0 or cpu < 0:
        raise ValueError('invalid phase samples')
    return {'wall_seconds': elapsed, 'cpu_seconds': cpu, 'observed_cpu_percent': cpu / elapsed * 100,
            'start': start, 'end': end}


class Owner:
    def __init__(self, binary, data, resource):
        self.resource = resource
        self.process = subprocess.Popen(['/usr/bin/time', '-l', '-o', str(resource), str(binary),
                                          '--mdreader-service', str(data)], stdin=subprocess.DEVNULL,
                                         stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.client = bench.ServiceClient(data)
        self.pid = None
        started = time.monotonic()
        while True:
            try:
                self.pid = self.client.call('ping', timeout=.3)['pid']
                break
            except (OSError, ValueError, KeyError):
                if self.process.poll() is not None or time.monotonic() - started > 20:
                    self.close()
                    raise RuntimeError('synthetic packaged owner failed to start')
                time.sleep(.02)
        self.startup_ms = (time.monotonic() - started) * 1000

    def close(self):
        if self.process.poll() is not None:
            self.pid = None
            return bench.parse_resource(self.resource.read_text()) if self.resource.exists() else {}
        if self.pid:
            try:
                os.kill(self.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        else:
            self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            if self.pid:
                try:
                    os.kill(self.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            self.process.kill()
            self.process.wait(timeout=5)
        return bench.parse_resource(self.resource.read_text()) if self.resource.exists() else {}


def pause_window(owner, source, seconds):
    # Pause cannot cancel one already-running bounded inference. Let it finish
    # and install before counting the no-new-work window.
    time.sleep(.35)
    before = owner.client.call('status', {'sources': [source]})
    start = sample(owner.pid)
    time.sleep(seconds)
    after = owner.client.call('status', {'sources': [source]})
    result = summarize(start, sample(owner.pid))
    result.update(before=before, after=after, no_new_vectors=before['embedded'] == after['embedded'])
    return result


def run(args):
    binary = args.binary.resolve(strict=True)
    cache = args.model_cache.resolve(strict=True)
    binary_hash = bench.digest(binary)
    result = {'kind': 'actual-bge-short-resource-probe', 'binary_sha256': binary_hash,
              'binary_bytes': binary.stat().st_size, 'harness_sha256': bench.digest(Path(__file__)),
              'helper_sha256': bench.digest(HELPER), 'platform': platform.platform(),
              'synthetic_only': True, 'documents': args.documents, 'actual_bge': False,
              'configured_onnx_intra_threads': 1,
              'thread_configuration_evidence': 'parent-supplied native build includes BgeEmbedder.with_intra_threads(1)',
              'balanced_target_inference_wall_duty': .25,
              'actor_duty_fraction_measured': False,
              'host_contention': 'Shared macOS host; unrelated AgentHero builds reported. Root Cargo paused during probe.',
              'failures': []}
    started = time.monotonic()
    owner = None
    try:
        with tempfile.TemporaryDirectory(prefix='mdreader-resource-probe-') as temporary, contextlib.ExitStack() as cleanup:
            root = Path(temporary)
            data, notes = root / 'private library', root / 'synthetic worktree'
            data.mkdir(mode=0o700)
            notes.mkdir()
            documents, _ = bench.corpus(notes, args.documents)
            checksum = hashlib.sha256()
            for index, (path, markdown) in enumerate(documents.items()):
                fact = bench.TOPICS[index % len(bench.TOPICS)][-1]
                markdown += '\n' + '\n\n'.join(f'Observation {n + 1}: {fact}' for n in range(3)) + '\n'
                (notes / path).write_text(markdown)
                documents[path] = markdown
                checksum.update(path.encode() + b'\0' + markdown.encode() + b'\0')
            result.update(corpus_sha256=checksum.hexdigest(), corpus_bytes=sum(len(x.encode()) for x in documents.values()))
            (data / 'models').symlink_to(cache, target_is_directory=True)
            owner = Owner(binary, data, root / 'first-resource.txt')
            def cleanup_owner():
                nonlocal owner
                if owner is not None:
                    result['interrupted_process_resources'] = owner.close()
                    owner = None
            cleanup.callback(cleanup_owner)
            result['startup_ms'] = owner.startup_ms
            client = owner.client
            result['fresh_default_policy'] = client.call('embedding_policy')
            if result['fresh_default_policy'] != 'balanced':
                raise ValueError('fresh product default is not balanced')
            identity = notes.stat()
            source = client.call('admit', {'name': 'Synthetic resource probe', 'path': str(notes),
                'identity': {'platform': 'unix', 'device': identity.st_dev, 'inode': identity.st_ino}})['id']
            client.call('index', {'source_id': source})
            first = {'query': bench.TOPICS[0][3], 'sources': [source], 'limit': 10, 'mode': 'semantic'}
            model_start = time.monotonic()
            while True:
                outcome = client.call('search_ranked', first)
                if outcome['semantic_unavailable'] is None:
                    result['actual_bge'] = True
                    break
                if time.monotonic() - model_start > 130 or 'restart' in outcome['semantic_unavailable'].lower():
                    raise RuntimeError('actual model initialization failed')
                time.sleep(.05)
            result['model_ready_ms'] = (time.monotonic() - model_start) * 1000
            client.call('start_index', {'source_id': source})
            # Observe background work alone before foreground queries add CPU.
            before = client.call('status', {'sources': [source]})
            phase = sample(owner.pid)
            time.sleep(.75)
            after = client.call('status', {'sources': [source]})
            result['balanced_background_only'] = {**summarize(phase, sample(owner.pid)), 'before': before, 'after': after}
            result['foreground_during_background'] = []
            phase = sample(owner.pid)
            for index in range(9):
                mode = ('exact', 'semantic', 'hybrid')[index % 3]
                topic, _, keyword, paraphrase, _ = bench.TOPICS[index % len(bench.TOPICS)]
                query_start = time.monotonic()
                outcome = client.call('search_ranked', {'query': keyword if mode == 'exact' else paraphrase,
                    'sources': [source], 'limit': 10, 'mode': mode})
                elapsed = (time.monotonic() - query_start) * 1000
                for hit in outcome['hits']:
                    bench.validate_hit(hit, source, documents)
                result['foreground_during_background'].append({'mode': mode, 'elapsed_ms': elapsed,
                    'semantic_unavailable': outcome['semantic_unavailable'], 'hits': len(outcome['hits']),
                    'actual_semantic_evidence': any(h.get('semantic_rank') for h in outcome['hits']),
                    'job_phase': client.call('job_status', {'source_id': source})['phase']})
                time.sleep(.025)
            result['foreground_phase_resources'] = summarize(phase, sample(owner.pid))
            client.call('set_embedding_policy', {'policy': 'paused'})
            result['paused'] = pause_window(owner, source, 5.)
            if not result['paused']['no_new_vectors']:
                raise ValueError('vectors advanced during settled pause window')
            if result['paused']['after']['embedded'] >= result['paused']['after']['chunks']:
                raise ValueError('fixture completed too early to exercise pending paused work')
            # A real foreground embedding bypasses pause, with existing partial vectors.
            query_start = time.monotonic()
            outcome = client.call('search_ranked', first)
            result['foreground_while_paused'] = {'elapsed_ms': (time.monotonic() - query_start) * 1000,
                'semantic_unavailable': outcome['semantic_unavailable'],
                'actual_semantic_evidence': any(h.get('semantic_rank') for h in outcome['hits'])}
            if outcome['semantic_unavailable'] is not None or not result['foreground_while_paused']['actual_semantic_evidence']:
                raise ValueError('foreground actual semantic query did not bypass pause')
            result['first_process_resources'] = owner.close()
            owner = None
            owner = Owner(binary, data, root / 'restart-resource.txt')
            client = owner.client
            result['restart_policy'] = client.call('embedding_policy')
            if result['restart_policy'] != 'paused':
                raise ValueError('pause did not persist across restart')
            result['restart_paused'] = pause_window(owner, source, 3.)
            if not result['restart_paused']['no_new_vectors']:
                raise ValueError('restart resumed paused vector work')
            client.call('set_embedding_policy', {'policy': 'balanced'})
            phase = sample(owner.pid)
            completion_start = time.monotonic()
            while True:
                job = client.call('job_status', {'source_id': source})
                if job['phase'] == 'done':
                    break
                if job['phase'] in ('error', 'model-unavailable') or time.monotonic() - completion_start > 60:
                    raise RuntimeError('small resumed actual BGE job failed or exceeded bound')
                time.sleep(.1)
            result['resumed_balanced'] = summarize(phase, sample(owner.pid))
            result['fully_indexed_status'] = client.call('status', {'sources': [source]})
            if result['fully_indexed_status']['embedded'] != result['fully_indexed_status']['chunks']:
                raise ValueError('resumed job lacks actual passage vectors')
            # Model remains loaded, but there is no index work or query workload.
            time.sleep(.3)
            phase = sample(owner.pid)
            time.sleep(8.)
            result['loaded_idle'] = summarize(phase, sample(owner.pid))
            result['restart_process_resources'] = owner.close()
            owner = None
    except (OSError, ValueError, RuntimeError, KeyError, TimeoutError) as error:
        result['failures'].append(str(error))
    except KeyboardInterrupt:
        result['failures'].append('probe interrupted deliberately')
    finally:
        if owner is not None:
            result['interrupted_process_resources'] = owner.close()
    result['binary_unchanged'] = bench.digest(binary) == binary_hash
    result['total_wall_seconds'] = time.monotonic() - started
    result['passed_control_checks'] = result['actual_bge'] and result['binary_unchanged'] and not result['failures']
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'receipt': str(args.output), 'passed_control_checks': result['passed_control_checks'], 'failures': result['failures']}), flush=True)
    return 0 if result['passed_control_checks'] else 1


class Tests(unittest.TestCase):
    def test_process_cpu_time_units_and_long_uptime(self):
        self.assertAlmostEqual(cpu_seconds('0:01.25'), 1.25)
        self.assertAlmostEqual(cpu_seconds('1-02:03:04.50'), 93784.5)
        with self.assertRaises(ValueError):
            cpu_seconds('1:2:3:4')

    def test_darwin_thread_columns_are_parsed_without_counting_headers(self):
        raw = 'USER PID TT %CPU STAT\nsynthetic 42 ?? 0.0 S\nsynthetic 42 ?? 0.0 S\nother 43 ?? 0.0 S\n'
        self.assertEqual(process_threads(raw, 42), 2)
        self.assertIsNone(process_threads('unexpected output', 42))

    def test_phase_cpu_is_delta_not_lifetime_percent(self):
        start = {'monotonic_seconds': 10., 'cpu_seconds': 100.}
        end = {'monotonic_seconds': 14., 'cpu_seconds': 101.}
        self.assertEqual(summarize(start, end)['observed_cpu_percent'], 25.)
        with self.assertRaises(ValueError):
            summarize(end, start)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--model-cache', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--documents', type=int, default=120)
    parser.add_argument('--self-test', action='store_true')
    args = parser.parse_args()
    if args.self_test:
        return 0 if unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(Tests)).wasSuccessful() else 1
    if not args.binary or not args.model_cache or not args.output or not 20 <= args.documents <= 200:
        parser.error('explicit binary/cache/output and 20..200 synthetic documents required')
    if platform.system() != 'Darwin':
        parser.error('Darwin process resource sampling required')
    return run(args)


if __name__ == '__main__':
    raise SystemExit(main())
