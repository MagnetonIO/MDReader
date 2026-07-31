import { describe, expect, test } from 'vitest';

import {
  assertForwardStableVersion,
  createR2PutInput,
  publishReleaseEntries,
  type ConditionalReleaseStore,
  type R2ObjectState,
} from '../r2-publisher';
import type { PublicationEntry } from '../release-pipeline';

function entry(
  key: string,
  immutable: boolean,
): PublicationEntry {
  return {
    kind: immutable ? 'artifact' : 'pointer',
    key,
    immutable,
    localPath: null,
    contents: '{}\n',
  };
}

describe('R2 release publication', () => {
  test('uses an atomic create-only condition for every immutable release key', () => {
    const input = createR2PutInput(
      'mdreader-downloads',
      entry('releases/v1.0.0/MDReader-1.0.0.dmg', true),
      Buffer.from('artifact'),
    );

    expect(input).toMatchObject({
      Bucket: 'mdreader-downloads',
      Key: 'releases/v1.0.0/MDReader-1.0.0.dmg',
      IfNoneMatch: '*',
    });
  });

  test('creates the first channel pointer atomically', () => {
    const input = createR2PutInput(
      'mdreader-downloads',
      entry('channels/stable/manifest.json', false),
      Buffer.from('{}\n'),
      null,
    );

    expect(input).toMatchObject({
      Bucket: 'mdreader-downloads',
      Key: 'channels/stable/manifest.json',
      IfNoneMatch: '*',
    });
  });

  test('refuses an unconditional mutable pointer write', () => {
    expect(() =>
      createR2PutInput(
        'mdreader-downloads',
        entry('channels/stable/manifest.json', false),
        Buffer.from('{}\n'),
      ),
    ).toThrow(/ETag condition/);
  });

  test('advances an existing channel pointer with its exact ETag', () => {
    const input = createR2PutInput(
      'mdreader-downloads',
      entry('channels/stable/manifest.json', false),
      Buffer.from('{}\n'),
      '"etag-v1"',
    );
    expect(input).toMatchObject({ IfMatch: '"etag-v1"' });
    expect(input.IfNoneMatch).toBeUndefined();
  });

  test('refuses equal or older stable releases before any mutable write', () => {
    expect(() => assertForwardStableVersion('1.0.0', '1.0.0')).toThrow(/forward/);
    expect(() => assertForwardStableVersion('1.1.0', '1.0.0')).toThrow(/forward/);
    expect(() => assertForwardStableVersion('1.0.0', '1.0.1')).not.toThrow();
    expect(() => assertForwardStableVersion(null, '1.0.0')).not.toThrow();
  });

  test('uses numeric stable SemVer ordering and rejects every suffix policy violation', () => {
    expect(() => assertForwardStableVersion('1.9.0', '1.10.0')).not.toThrow();
    expect(() => assertForwardStableVersion('1.10.0', '1.9.0')).toThrow(
      /forward/,
    );
    for (const version of [
      '1.0.0-A',
      '1.0.0-a',
      '1.0.0-1',
      '1.0.0-01',
      '1.0.0+build.1',
      '01.0.0',
    ]) {
      expect(() => assertForwardStableVersion(null, version)).toThrow(
        /stable-channel semantic version/,
      );
    }
  });

  test('repairs every crash boundary without replacing equal immutable bytes or pointers', async () => {
    for (const failAt of [1, 2]) {
      const store = new MemoryStore(failAt);
      const entries = releaseEntries();

      await expect(
        publishReleaseEntries(entries, loadEntry, store),
      ).rejects.toThrow(/injected publication failure/);

      store.failAt = null;
      await expect(
        publishReleaseEntries(entries, loadEntry, store),
      ).resolves.toBeUndefined();
      await expect(
        publishReleaseEntries(entries, loadEntry, store),
      ).resolves.toBeUndefined();

      expect(text(store.objects.get('channels/stable/updater.json'))).toContain(
        '"version":"2.0.0"',
      );
      expect(text(store.objects.get('channels/stable/manifest.json'))).toContain(
        '"version":"2.0.0"',
      );
      expect(store.puts.filter((key) => key.includes('channels/stable'))).toEqual([
        'channels/stable/manifest.json',
        'channels/stable/updater.json',
      ]);
    }
  });

  test('rejects a same-version pointer unless its body is the exact canonical body', async () => {
    const store = new MemoryStore(null);
    store.seed(
      'channels/stable/updater.json',
      '{"version":"2.0.0","platforms":{"tampered":true}}\n',
    );

    await expect(
      publishReleaseEntries(releaseEntries(), loadEntry, store),
    ).rejects.toThrow(/same-version pointer.*does not match/);
    expect(store.puts).toEqual([]);
  });

  test('repairs a missing updater commit point after the manifest pointer reached the same version', async () => {
    const store = new MemoryStore(null);
    store.seed(
      'channels/stable/manifest.json',
      '{"version":"2.0.0","manifestSha256":"fixed","sourceCommit":"abc"}\n',
    );

    await expect(
      publishReleaseEntries(releaseEntries(), loadEntry, store),
    ).resolves.toBeUndefined();
    expect(store.puts).toEqual([
      'releases/v2.0.0/manifest.json',
      'channels/stable/updater.json',
    ]);
  });

  test('refuses divergent same-version updater bytes after the manifest pointer advanced', async () => {
    const store = new MemoryStore(null);
    store.seed(
      'channels/stable/manifest.json',
      '{"version":"2.0.0","manifestSha256":"fixed","sourceCommit":"abc"}\n',
    );
    store.seed(
      'channels/stable/updater.json',
      '{"version":"2.0.0","platforms":{"tampered":true}}\n',
    );

    await expect(
      publishReleaseEntries(releaseEntries(), loadEntry, store),
    ).rejects.toThrow(/same-version pointer.*does not match/);
    expect(store.puts).toEqual([]);
  });

  test('rejects a retry when any immutable release object differs', async () => {
    const store = new MemoryStore(null);
    store.seed('releases/v2.0.0/manifest.json', 'different immutable body');

    await expect(
      publishReleaseEntries(releaseEntries(), loadEntry, store),
    ).rejects.toThrow(/immutable release object.*different bytes/);
    expect(store.puts).toEqual([]);
  });

  test('a competing pointer writer wins the ETag CAS instead of being overwritten', async () => {
    const store = new MemoryStore(null);
    store.seed('channels/stable/updater.json', '{"version":"1.0.0"}\n');
    store.seed('channels/stable/manifest.json', '{"version":"1.0.0"}\n');
    store.competeOn = 'channels/stable/updater.json';

    await expect(
      publishReleaseEntries(releaseEntries(), loadEntry, store),
    ).rejects.toThrow(/precondition failed/);
    expect(text(store.objects.get('channels/stable/updater.json'))).toContain(
      '"version":"3.0.0"',
    );
  });

  test('gives the store each entry and exact desired byte length before preflight', async () => {
    const entries = releaseEntries();
    const observed: Array<[string, number]> = [];
    const store: ConditionalReleaseStore = {
      async read(
        entryOrKey: PublicationEntry | string,
        desiredBytes?: number,
      ): Promise<R2ObjectState | null> {
        if (typeof entryOrKey === 'string' || desiredBytes === undefined) {
          throw new Error('preflight read is missing its desired object capability');
        }
        observed.push([entryOrKey.key, desiredBytes]);
        return null;
      },
      async put(): Promise<void> {},
    };

    await expect(
      publishReleaseEntries(entries, loadEntry, store),
    ).resolves.toBeUndefined();
    expect(observed).toEqual(
      entries.map((item) => [
        item.key,
        Buffer.byteLength(item.contents ?? ''),
      ]),
    );
  });
});

function releaseEntries(): PublicationEntry[] {
  return [
    {
      kind: 'manifest',
      key: 'releases/v2.0.0/manifest.json',
      immutable: true,
      localPath: null,
      contents:
        '{"schemaVersion":1,"sourceCommit":"abc","releases":[{"version":"2.0.0","checksums":"fixed"}]}\n',
    },
    {
      kind: 'pointer',
      key: 'channels/stable/manifest.json',
      immutable: false,
      localPath: null,
      contents:
        '{"version":"2.0.0","manifestSha256":"fixed","sourceCommit":"abc"}\n',
    },
    {
      kind: 'updater-manifest',
      key: 'channels/stable/updater.json',
      immutable: false,
      localPath: null,
      contents: '{"version":"2.0.0","platforms":{"darwin-aarch64":"fixed"}}\n',
    },
  ];
}

async function loadEntry(entry: PublicationEntry): Promise<Uint8Array> {
  return Buffer.from(entry.contents ?? '');
}

function text(state: R2ObjectState | undefined): string {
  if (!state) throw new Error('expected object');
  return Buffer.from(state.body).toString('utf8');
}

class MemoryStore implements ConditionalReleaseStore {
  readonly objects = new Map<string, R2ObjectState>();
  readonly puts: string[] = [];
  failAt: number | null;
  competeOn: string | null = null;
  private etag = 0;
  private mutablePuts = 0;

  constructor(failAt: number | null) {
    this.failAt = failAt;
  }

  seed(key: string, body: string): void {
    this.objects.set(key, {
      body: Buffer.from(body),
      etag: `"etag-${this.etag++}"`,
    });
  }

  async read(entry: PublicationEntry): Promise<R2ObjectState | null> {
    return this.objects.get(entry.key) ?? null;
  }

  async put(
    entry: PublicationEntry,
    body: Uint8Array,
    expectedEtag?: string | null,
  ): Promise<void> {
    if (!entry.immutable) {
      this.mutablePuts += 1;
      if (this.failAt === this.mutablePuts) {
        throw new Error('injected publication failure');
      }
    }
    if (this.competeOn === entry.key) {
      this.competeOn = null;
      this.seed(entry.key, '{"version":"3.0.0","publisher":"competitor"}\n');
    }
    const current = this.objects.get(entry.key);
    if (entry.immutable) {
      if (current) throw new Error('precondition failed: immutable exists');
    } else if (
      (expectedEtag === null && current) ||
      (typeof expectedEtag === 'string' && current?.etag !== expectedEtag)
    ) {
      throw new Error('precondition failed: ETag mismatch');
    }
    this.objects.set(entry.key, {
      body: Buffer.from(body),
      etag: `"etag-${this.etag++}"`,
    });
    this.puts.push(entry.key);
  }
}
