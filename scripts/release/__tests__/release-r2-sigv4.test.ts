import { createHash } from 'node:crypto';

import { describe, expect, test } from 'vitest';

import { createSignedR2Request } from '../r2-sigv4';

const CREDENTIALS = {
  accountId: '0123456789abcdef0123456789abcdef',
  accessKeyId: 'test-access-key',
  secretAccessKey: 'test-secret-key',
};

describe('sealed publisher R2 signing', () => {
  test('signs the exact path, payload, and conditional headers without exposing the secret', () => {
    const body = Buffer.from('release body');
    const request = createSignedR2Request(
      CREDENTIALS,
      'mdreader-downloads',
      'PUT',
      'releases/v1.10.0/file name.json',
      body,
      {
        'content-type': 'application/json',
        'if-match': '"etag 7"',
      },
      new Date('2026-07-26T12:34:56.000Z'),
    );
    const headers = new Headers(request.init.headers);

    expect(request.url.toString()).toBe(
      'https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com/mdreader-downloads/releases/v1.10.0/file%20name.json',
    );
    expect(headers.get('x-amz-date')).toBe('20260726T123456Z');
    expect(headers.get('x-amz-content-sha256')).toBe(
      createHash('sha256').update(body).digest('hex'),
    );
    expect(headers.get('authorization')).toContain(
      'Credential=test-access-key/20260726/auto/s3/aws4_request',
    );
    expect(headers.get('authorization')).toContain(
      'SignedHeaders=content-type;host;if-match;x-amz-content-sha256;x-amz-date',
    );
    expect(headers.get('authorization')).not.toContain(
      CREDENTIALS.secretAccessKey,
    );
  });

  test('binds the signature to the exact payload and conditional write', () => {
    const fixed = new Date('2026-07-26T12:34:56.000Z');
    const signed = (body: string, etag: string) =>
      new Headers(
        createSignedR2Request(
          CREDENTIALS,
          'mdreader-downloads',
          'PUT',
          'channels/stable/updater.json',
          Buffer.from(body),
          { 'if-match': etag },
          fixed,
        ).init.headers,
      ).get('authorization');

    expect(signed('one', '"etag-1"')).not.toBe(signed('two', '"etag-1"'));
    expect(signed('one', '"etag-1"')).not.toBe(signed('one', '"etag-2"'));
  });

  test.each([
    'user@evil.example/',
    '0123456789abcdef',
    '0123456789ABCDEF0123456789ABCDEF',
    '0123456789abcdef0123456789abcdeg',
    '0123456789abcdef0123456789abcdef.evil.example',
  ])('refuses a noncanonical Cloudflare account ID before signing: %s', (accountId) => {
    expect(() =>
      createSignedR2Request(
        { ...CREDENTIALS, accountId },
        'mdreader-downloads',
        'GET',
        'channels/stable/manifest.json',
        new Uint8Array(),
        {},
      ),
    ).toThrow(/Cloudflare account ID/);
  });
});
