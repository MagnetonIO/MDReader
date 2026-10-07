#!/usr/bin/env python3
"""Measure real BGE via a packaged service and a disposable synthetic corpus.

Only the explicit public model cache survives. No installed profiles, user
sources, configuration, or histories are read. Results never contain auth tokens.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import platform
import re
import signal
import socket
import subprocess
import tempfile
import threading
import time
import unittest

MAX_RESPONSE = 16 * 1024 * 1024
TOPICS = [
    ('solar', 'Photovoltaic rooftop energy', 'photovoltaic', 'generate electricity from sunshine',
     'Photovoltaic panels convert sunlight into electrical power for homes. Rooftop solar arrays connect an inverter to the household electrical circuit. Shading reduces energy production and panel output.'),
    ('bread', 'Sourdough fermentation', 'sourdough', 'bake a loaf using a living flour starter',
     'Sourdough bread uses wild yeast and lactic acid bacteria in a flour starter. Fermentation develops flavor and raises dough before baking. Feed the starter with water and fresh flour.'),
    ('security', 'Password authentication', 'password', 'protect a login account with a second verification factor',
     'Password authentication protects user accounts. Multifactor verification adds a hardware security key or one time code. A password manager creates unique credentials and prevents reuse.'),
    ('garden', 'Vegetable garden compost', 'compost', 'turn kitchen scraps into food for vegetable plants',
     'Compost recycles kitchen scraps and garden leaves into nutrients for vegetable plants. Decomposition requires moisture, oxygen and a balanced mixture of brown and green material.'),
    ('astronomy', 'Planetary telescope observations', 'telescope', 'observe distant planets through an optical instrument',
     'A telescope gathers light from distant planets and stars. Astronomical observers adjust focus and magnification, avoid atmospheric turbulence, and track planets across the night sky.'),
    ('batteries', 'Lithium battery charging', 'lithium', 'store electrical energy in a rechargeable cell',
     'Lithium ion batteries store electrical energy through reversible chemical reactions. Charging transfers ions between electrodes. Thermal management and charge limits protect the rechargeable cell.'),
    ('bridges', 'Suspension bridge engineering', 'suspension', 'carry traffic across a river using cables and towers',
     'Suspension bridges support a road deck with cables hung from tall towers. Anchors resist cable tension, while the towers carry compression. Engineers design for traffic loads and wind.'),
    ('piano', 'Piano practice techniques', 'piano', 'learn to play a keyboard instrument with both hands',
     'Piano practice develops coordination between both hands on a musical keyboard. Slow scales, correct fingering, and a metronome help build rhythm before increasing the tempo.'),
    ('water', 'Drinking water filtration', 'filtration', 'remove impurities from water before people drink it',
     'Drinking water filtration removes particles and impurities. Activated carbon reduces odors, and membrane filters separate contaminants. Disinfection controls microbes before the water reaches a tap.'),
    ('rail', 'Railway signal control', 'railway', 'keep trains safely separated on a shared track',
     'Railway signals keep trains separated on a shared track. Block occupancy detection and interlocking prevent conflicting routes. Dispatchers coordinate schedules and safe movement through junctions.'),
    ('volcano', 'Volcanic eruption monitoring', 'volcanic', 'watch changes in a mountain that may release molten rock',
     'Volcanic eruption monitoring measures earthquakes, ground deformation and gas emissions. Rising magma can swell a mountain before molten rock escapes. Scientists combine observations to assess hazards.'),
    ('backup', 'Disaster recovery backups', 'backups', 'restore computer files after a storage device fails',
     'Backups preserve copies of computer files for disaster recovery. An offsite copy protects against fire and theft. Regular restore tests confirm that stored data survives a storage device failure.'),
    ('migration', 'Migratory bird navigation', 'migratory', 'how seasonal birds find their way between distant habitats',
     'Migratory birds travel between distant habitats as seasons change. They navigate using the sun, stars, landmarks and magnetic fields. Resting sites provide food during the long journey.'),
    ('compiler', 'Compiler optimization passes', 'compiler', 'translate a programming language into efficient machine instructions',
     'A compiler translates source code into machine instructions. Parsing builds syntax trees, and optimization passes eliminate redundant computations. Register allocation maps intermediate values to processor registers.'),
    ('wildfire', 'Forest wildfire prevention', 'wildfire', 'reduce the chance that a forest fire spreads to homes',
     'Wildfire prevention reduces dry fuel near forests and homes. Controlled burns and vegetation management limit fire intensity. Evacuation planning and weather monitoring help communities prepare.'),
    ('marine', 'Coral reef conservation', 'coral', 'protect underwater reef habitats from warming seas',
     'Coral reefs provide marine habitats for fish and other organisms. Warming seas can cause coral bleaching. Conservation reduces pollution, protects reef ecosystems and monitors ocean temperature.'),
    ('glacier', 'Glacier ice movement', 'glacier', 'why a large mass of mountain ice slowly flows downhill',
     'A glacier slowly flows downhill under its own weight. Ice deformation and sliding at the bed move the frozen mass. Snow accumulation and melting determine whether a mountain glacier advances.'),
    ('audio', 'Microphone audio recording', 'microphone', 'capture clear speech with a sound recording device',
     'A microphone converts sound pressure into an electrical audio signal. Careful placement and a quiet room improve speech recordings. Gain adjustment prevents clipping and excessive background noise.'),
    ('soil', 'Soil erosion control', 'erosion', 'stop rain from washing fertile ground down a hillside',
     'Soil erosion removes fertile ground when rainfall runs down a hillside. Cover crops, terraces and contour planting slow runoff. Plant roots hold soil in place and protect agricultural land.'),
    ('vaccines', 'Vaccine immune memory', 'vaccine', 'prepare the immune system to recognize a pathogen before infection',
     'A vaccine prepares immune memory to recognize a pathogen. Antigen exposure trains immune cells without the full disease. Later infection triggers a faster protective response from the immune system.'),
]


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def tree_size(path):
    """Logical bytes and deduplicated allocated file bytes, without directory-link traversal."""
    logical = allocated = count = 0
    seen = set()
    for directory, _, names in os.walk(path, followlinks=False):
        for name in names:
            file = Path(directory) / name
            info = file.stat()
            logical += info.st_size
            count += 1
            identity = (info.st_dev, info.st_ino)
            if identity not in seen:
                allocated += info.st_blocks * 512
                seen.add(identity)
    return {'logical_bytes': logical, 'allocated_unique_bytes': allocated, 'files': count}


def corpus(directory, count):
    documents = {}
    checksum = hashlib.sha256()
    for index in range(count):
        topic, title, _, _, body = TOPICS[index % len(TOPICS)]
        relative = f'{topic}/case-{index:05}.md'
        # One Markdown section/chunk/document; differing observations prevent
        # identical-text fixtures while keeping the expected topic meaningful.
        markdown = (f'---\ntags: [{topic}, synthetic]\n---\n# {title}\n\n{body}\n\n'
                    f'Observation case{index:05}: field site {index // len(TOPICS):04}; '
                    f'measurement {(index * 37) % 997}; review cycle {index % 13 + 1}.\n')
        file = directory / relative
        file.parent.mkdir(exist_ok=True)
        file.write_text(markdown)
        documents[relative] = markdown
        checksum.update(relative.encode() + b'\0' + markdown.encode() + b'\0')
    return documents, checksum.hexdigest()


class ServiceClient:
    def __init__(self, data):
        self.data = data

    def call(self, method, params=None, timeout=10):
        # Endpoint is private synthetic state. Never return/store its token.
        endpoint = json.loads((self.data / 'reader-endpoint.json').read_text())
        if endpoint['protocol'] != 1 or not isinstance(endpoint['port'], int):
            raise ValueError('unexpected service endpoint protocol')
        request = {'method': method}
        if params is not None:
            request['params'] = params
        frame = {'protocol': 1, 'token': endpoint['token'], 'request': request}
        with socket.create_connection(('127.0.0.1', endpoint['port']), timeout=timeout) as stream:
            stream.sendall(json.dumps(frame).encode() + b'\n')
            raw = stream.makefile('rb').readline(MAX_RESPONSE + 1)
        if len(raw) > MAX_RESPONSE or not raw.endswith(b'\n'):
            raise ValueError('service response exceeds frame budget or is incomplete')
        response = json.loads(raw)
        if response.get('error'):
            raise RuntimeError(f"service error: {response['error']['code']}")
        return response['result']


def stats(samples):
    values = sorted(samples)
    if not values:
        return {'count': 0, 'p50_ms': None, 'p95_ms': None, 'max_ms': None}
    return {'count': len(values), 'p50_ms': values[math.ceil(.5 * len(values)) - 1],
            'p95_ms': values[math.ceil(.95 * len(values)) - 1], 'max_ms': values[-1]}


def validate_hit(hit, source, documents):
    path = hit.get('path', '')
    parts = PurePosixPath(path)
    if parts.is_absolute() or '..' in parts.parts or path not in documents or hit.get('source_id') != source:
        raise ValueError('result escaped the explicit synthetic source')
    revision = hit.get('revision')
    if not isinstance(revision, str) or not re.fullmatch('[0-9a-f]{64}', revision):
        raise ValueError('missing revision evidence')
    start, end = hit.get('line_start', 0), hit.get('line_end', 0)
    if not 1 <= start <= end <= len(documents[path].splitlines()):
        raise ValueError('invalid line evidence')
    if not hit.get('text') or hit['text'].strip() not in documents[path]:
        raise ValueError('retrieved text does not belong to source bytes')


def progress(event, **fields):
    print(json.dumps({'event': event, **fields}), flush=True)


def parse_resource(text):
    result = {'raw': text.strip()}
    timing = re.search(r'([\d.]+) real\s+([\d.]+) user\s+([\d.]+) sys', text)
    if timing:
        result.update(wall_seconds=float(timing[1]), user_cpu_seconds=float(timing[2]), system_cpu_seconds=float(timing[3]))
    maximum = re.search(r'(\d+)\s+maximum resident set size', text)
    if maximum:
        result['max_rss_bytes'] = int(maximum[1])  # Darwin time -l reports bytes.
    return result


def run(args):
    binary = args.binary.resolve(strict=True)
    cache = args.model_cache.resolve(strict=True)
    binary_hash = digest(binary)
    app = args.bundle.resolve(strict=True) if args.bundle else next((parent for parent in binary.parents if parent.suffix == '.app'), None)
    if app and digest(app / 'Contents/MacOS/mdreader-app') != binary_hash:
        raise ValueError('bundle executable does not match the immutable benchmark binary')
    receipt = {'kind': 'actual-bge-packaged-service-scale', 'documents_requested': args.documents,
               'binary_sha256': binary_hash, 'binary_bytes': binary.stat().st_size,
               'harness_sha256': digest(Path(__file__)),
               'bundle': tree_size(app) if app else None, 'model_cache_before': tree_size(cache),
               'platform': platform.platform(), 'machine': platform.machine(), 'build_class': args.build_class,
               'actual_bge': False, 'touches_installed_profile': False, 'synthetic_only': True,
               'query_samples_per_mode': args.samples, 'query_warmups_per_mode': args.warmups,
               'thresholds_ms': {'exact_p95': 150, 'hybrid_p95': 1000}, 'failures': [], 'phases': []}
    started = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(prefix='mdreader-bge-scale-') as temporary:
            root = Path(temporary)
            data = root / 'private library'
            notes = root / 'synthetic worktree'
            data.mkdir(mode=0o700)
            notes.mkdir()
            documents, checksum = corpus(notes, args.documents)
            receipt.update(corpus_sha256=checksum, corpus_bytes=sum(len(value.encode()) for value in documents.values()),
                           fixture_topics=len(TOPICS), one_section_per_document=True)
            # Explicitly opted-in public model cache, identical to packaged
            # acceptance. Source-root and production cache checks remain enabled.
            (data / 'models').symlink_to(cache, target_is_directory=True)
            resource_file = root / 'process-resources.txt'
            service = subprocess.Popen(['/usr/bin/time', '-l', '-o', str(resource_file), str(binary),
                                        '--mdreader-service', str(data)], stdin=subprocess.DEVNULL,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            client = ServiceClient(data)
            owner_pid = None
            try:
                launch = time.monotonic()
                while True:
                    try:
                        ping = client.call('ping', timeout=.5)
                        owner_pid = ping['pid']
                        break
                    except (OSError, ValueError, KeyError):
                        if service.poll() is not None or time.monotonic() - launch > 20:
                            raise RuntimeError('packaged service startup failed')
                        time.sleep(.02)
                receipt['startup_ms'] = (time.monotonic() - launch) * 1000
                # Throughput runs must opt in. Balanced is the actual product
                # default; the selected policy is part of every new receipt.
                selected_policy = client.call('set_embedding_policy', {'policy': args.embedding_policy})
                if selected_policy != args.embedding_policy:
                    raise RuntimeError('service did not acknowledge requested embedding policy')
                receipt['embedding_policy'] = selected_policy
                identity = notes.stat()
                source = client.call('admit', {'name': 'Synthetic scale corpus', 'path': str(notes),
                    'identity': {'platform': 'unix', 'device': identity.st_dev, 'inode': identity.st_ino}})['id']
                begin = time.monotonic()
                client.call('index', {'source_id': source}, timeout=120)
                receipt['cold_lexical_index_ms'] = (time.monotonic() - begin) * 1000
                lexical_status = client.call('status', {'sources': [source]})
                receipt['lexical_status'] = lexical_status
                if lexical_status['documents'] != args.documents or lexical_status['embedded'] != 0:
                    raise RuntimeError('unexpected initial corpus/index counts')
                receipt['index_before_vectors'] = tree_size(data)
                progress('lexical_indexed', documents=args.documents, milliseconds=receipt['cold_lexical_index_ms'])
                # Initialize separately from vector indexing, through the actual
                # query actor. Ready means a successful actual query embedding,
                # even though this fresh index has no passage vectors yet.
                first = {'query': TOPICS[0][3], 'sources': [source], 'limit': 10, 'mode': 'semantic'}
                begin = time.monotonic()
                while True:
                    outcome = client.call('search_ranked', first)
                    if outcome['semantic_unavailable'] is None:
                        receipt['actual_bge'] = True
                        break
                    if time.monotonic() - begin > 130 or 'restart' in outcome['semantic_unavailable'].lower():
                        raise RuntimeError('model initialization failed: ' + outcome['semantic_unavailable'])
                    time.sleep(.1)
                receipt['model_ready_and_first_query_ms'] = (time.monotonic() - begin) * 1000
                progress('model_ready', milliseconds=receipt['model_ready_and_first_query_ms'])
                begin = time.monotonic()
                client.call('start_index', {'source_id': source})
                last_phase = None
                poll = 0
                while True:
                    job = client.call('job_status', {'source_id': source}, timeout=30)
                    elapsed = time.monotonic() - begin
                    if job['phase'] != last_phase:
                        receipt['phases'].append({'phase': job['phase'], 'elapsed_ms': elapsed * 1000,
                                                  'embedded_reported': job['embedded']})
                        last_phase = job['phase']
                        progress('index_phase', phase=last_phase, elapsed_seconds=round(elapsed, 2))
                    if job['phase'] in ('error', 'model-unavailable'):
                        raise RuntimeError('actual vector indexing failed: ' + (job.get('error') or job['phase']))
                    if job['phase'] == 'done':
                        break
                    if elapsed > args.index_timeout:
                        receipt['index_timeout_job'] = job
                        raise TimeoutError('actual vector indexing exceeded configured wall deadline')
                    if poll % 20 == 0:
                        current = client.call('status', {'sources': [source]}, timeout=30)
                        progress('index_progress', elapsed_seconds=round(elapsed, 2), embedded=current['embedded'], chunks=current['chunks'])
                    poll += 1
                    time.sleep(.5)
                receipt['queued_index_and_embedding_ms'] = (time.monotonic() - begin) * 1000
                indexed = client.call('status', {'sources': [source]})
                receipt['fully_indexed_status'] = indexed
                if indexed['embedded'] != indexed['chunks'] or indexed['documents'] != args.documents:
                    raise RuntimeError('completed job lacks real vectors for all corpus chunks')
                progress('vectors_indexed', **indexed, elapsed_ms=receipt['queued_index_and_embedding_ms'])
                receipt['measurements'] = {}
                # Serial queries, after full passage embedding, with per-mode
                # warmups. Network framing and service query embedding included.
                for mode in ('exact', 'semantic', 'hybrid'):
                    samples = []
                    for index in range(-args.warmups, args.samples):
                        topic, _, keyword, paraphrase, _ = TOPICS[index % len(TOPICS)]
                        query = keyword if mode == 'exact' else paraphrase
                        begin_query = time.monotonic()
                        failure = None
                        try:
                            outcome = client.call('search_ranked', {'query': query, 'sources': [source], 'limit': 10, 'mode': mode}, timeout=30)
                            elapsed_ms = (time.monotonic() - begin_query) * 1000
                            hits = outcome['hits']
                            for hit in hits:
                                validate_hit(hit, source, documents)
                            expected = next((position + 1 for position, hit in enumerate(hits) if hit['path'].startswith(topic + '/')), None)
                            if outcome['semantic_unavailable'] is not None:
                                failure = outcome['semantic_unavailable']
                            elif expected is None or expected > 5:
                                failure = 'expected topic missing from top five'
                            elif mode != 'exact' and not any(hit.get('semantic_rank') for hit in hits):
                                failure = 'no actual semantic rank evidence'
                            sample = {'query': query, 'elapsed_ms': elapsed_ms, 'hits': len(hits), 'expected_topic_rank': expected,
                                      'semantic_unavailable': outcome['semantic_unavailable'], 'failure': failure,
                                      'top': [{key: hit.get(key) for key in ('path','revision','line_start','line_end','score','lexical_rank','semantic_rank')} for hit in hits[:3]]}
                            if hits and index >= 0:
                                # Validate the source revision through service,
                                # outside the timed search sample.
                                top = hits[0]
                                opened = client.call('read', {'source_id': source, 'path': top['path'], 'revision': top['revision']})
                                if opened['markdown'] != documents[top['path']]:
                                    raise ValueError('read evidence differs from original source bytes')
                        except (OSError, ValueError, RuntimeError, KeyError) as error:
                            sample = {'query': query, 'elapsed_ms': (time.monotonic() - begin_query) * 1000,
                                      'failure': str(error), 'hits': 0}
                        if index >= 0:
                            samples.append(sample)
                    summary = stats([sample['elapsed_ms'] for sample in samples])
                    summary.update(failed_samples=sum(sample['failure'] is not None for sample in samples), samples=samples)
                    receipt['measurements'][mode] = summary
                    progress('query_mode_complete', mode=mode, p95_ms=summary['p95_ms'], failed_samples=summary['failed_samples'])
                receipt['index_with_vectors'] = tree_size(data)
                receipt['sqlite_files'] = {file.name: file.stat().st_size for file in data.glob('*.sqlite*')}
            finally:
                if owner_pid:
                    try:
                        os.kill(owner_pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                else:
                    service.terminate()
                try:
                    service.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    if owner_pid:
                        os.kill(owner_pid, signal.SIGKILL)
                    service.kill()
                    service.wait(timeout=5)
                if resource_file.exists():
                    receipt['service_process_resources'] = parse_resource(resource_file.read_text())
    except KeyboardInterrupt:
        receipt['failures'].append('Interrupted deliberately before scale acceptance completed')
    except (OSError, ValueError, RuntimeError, KeyError, TimeoutError) as error:
        receipt['failures'].append(str(error))
    receipt['model_cache_after'] = tree_size(cache)
    receipt['binary_unchanged'] = digest(binary) == binary_hash
    receipt['total_wall_seconds'] = time.monotonic() - started
    measures = receipt.get('measurements', {})
    clean = not receipt['failures'] and receipt['binary_unchanged'] and receipt['actual_bge']
    receipt['accepts_lexical_budget'] = clean and measures.get('exact', {}).get('failed_samples') == 0 and measures['exact']['p95_ms'] < 150
    receipt['accepts_hybrid_budget'] = clean and measures.get('hybrid', {}).get('failed_samples') == 0 and measures['hybrid']['p95_ms'] < 1000
    receipt['accepts_10k_hybrid_budget'] = args.documents == 10000 and receipt['accepts_hybrid_budget']
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(receipt, indent=2) + '\n')
    progress('receipt_written', output=str(args.output), accepts_hybrid_budget=receipt['accepts_hybrid_budget'], failures=receipt['failures'])
    return 0 if clean else 1


class FixtureTests(unittest.TestCase):
    def test_corpus_is_deterministic_and_one_section_per_document(self):
        with tempfile.TemporaryDirectory() as left, tempfile.TemporaryDirectory() as right:
            first, checksum = corpus(Path(left), 43)
            second, other = corpus(Path(right), 43)
            self.assertEqual(first, second)
            self.assertEqual(checksum, other)
            self.assertEqual(len(first), 43)
            self.assertTrue(all(value.count('\n# ') == 1 for value in first.values()))

    def test_evidence_guards_source_paths_revisions_and_lines(self):
        documents = {'solar/a.md': '# Solar\nEvidence\n'}
        valid = {'source_id': 'allowed', 'path': 'solar/a.md', 'revision': 'a' * 64, 'line_start': 2, 'line_end': 2, 'text': 'Evidence'}
        validate_hit(valid, 'allowed', documents)
        for change in ({'source_id': 'other'}, {'path': '../outside'}, {'revision': None}, {'line_end': 99}, {'text': 'invented'}):
            with self.assertRaises((ValueError, TypeError)):
                validate_hit({**valid, **change}, 'allowed', documents)

    def test_loopback_envelope_and_bounded_response(self):
        def exchange(response):
            with tempfile.TemporaryDirectory() as temporary, socket.socket() as listener:
                listener.bind(('127.0.0.1', 0))
                listener.listen(1)
                data = Path(temporary)
                (data / 'reader-endpoint.json').write_text(json.dumps({'protocol': 1, 'port': listener.getsockname()[1], 'token': 'synthetic-token'}))
                received = []
                def serve():
                    with listener.accept()[0] as stream:
                        received.append(json.loads(stream.makefile('rb').readline(4096)))
                        stream.sendall(response)
                thread = threading.Thread(target=serve)
                thread.start()
                try:
                    return ServiceClient(data).call('ping', timeout=1), received
                finally:
                    thread.join(timeout=1)
                    self.assertFalse(thread.is_alive())
        result, received = exchange(b'{"result":{"ok":true},"error":null}\n')
        self.assertEqual(result, {'ok': True})
        self.assertEqual(received, [{'protocol': 1, 'token': 'synthetic-token', 'request': {'method': 'ping'}}])
        global MAX_RESPONSE
        original = MAX_RESPONSE
        try:
            MAX_RESPONSE = 64
            with self.assertRaisesRegex(ValueError, 'frame budget'):
                exchange(b'x' * 65 + b'\n')
        finally:
            MAX_RESPONSE = original

    def test_nearest_rank_percentiles_and_resource_units(self):
        self.assertEqual(stats(list(range(1, 21)))['p95_ms'], 19)
        self.assertEqual(stats([])['p95_ms'], None)
        resource = parse_resource('1.25 real 0.50 user 0.25 sys\n 12345 maximum resident set size\n')
        self.assertEqual(resource['max_rss_bytes'], 12345)
        self.assertEqual(resource['user_cpu_seconds'], .5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--self-test', action='store_true')
    parser.add_argument('--binary', type=Path)
    parser.add_argument('--bundle', type=Path, help='Optional matching app bundle for size accounting when binary is an immutable copy')
    parser.add_argument('--model-cache', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--documents', type=int, default=10000)
    parser.add_argument('--samples', type=int, default=30)
    parser.add_argument('--warmups', type=int, default=5)
    parser.add_argument('--index-timeout', type=float, default=1800)
    parser.add_argument('--build-class', default='debug packaged bundle')
    parser.add_argument('--embedding-policy', choices=('balanced', 'full_speed'), default='balanced')
    args = parser.parse_args()
    if args.self_test:
        result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromTestCase(FixtureTests))
        return 0 if result.wasSuccessful() else 1
    if not all((args.binary, args.model_cache, args.output)) or not 20 <= args.documents <= 50000 or args.samples < 20 or args.warmups < 1:
        parser.error('require explicit binary/model-cache/output, 20..50000 documents, >=20 samples and >=1 warmup')
    if platform.system() != 'Darwin':
        parser.error('resource collection currently requires Darwin /usr/bin/time -l')
    return run(args)


if __name__ == '__main__':
    raise SystemExit(main())
