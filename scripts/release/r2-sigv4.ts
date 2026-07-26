import { createHash, createHmac } from 'node:crypto';

export interface R2Credentials {
  accountId: string;
  accessKeyId: string;
  secretAccessKey: string;
}

function hexSha256(body: Uint8Array | string): string {
  return createHash('sha256').update(body).digest('hex');
}

function hmac(key: Uint8Array | string, value: string): Buffer {
  return createHmac('sha256', key).update(value).digest();
}

function awsEncode(value: string): string {
  return encodeURIComponent(value).replace(/[!'()*]/g, (character) =>
    `%${character.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

function objectUrl(accountId: string, bucket: string, key: string): URL {
  if (!/^[0-9a-f]{32}$/.test(accountId)) {
    throw new Error('Cloudflare account ID must be exactly 32 lowercase hexadecimal characters');
  }
  const path = [bucket, ...key.split('/')].map(awsEncode).join('/');
  const url = new URL(
    `https://${accountId}.r2.cloudflarestorage.com/${path}`,
  );
  if (
    url.origin !==
    `https://${accountId}.r2.cloudflarestorage.com`
  ) {
    throw new Error('Cloudflare account ID produced an unexpected R2 origin');
  }
  return url;
}

export function createSignedR2Request(
  credentials: R2Credentials,
  bucket: string,
  method: 'GET' | 'PUT',
  key: string,
  body: Uint8Array,
  extraHeaders: Record<string, string>,
  now = new Date(),
): { url: URL; init: RequestInit } {
  const url = objectUrl(credentials.accountId, bucket, key);
  const amzDate = now.toISOString().replace(/[:-]|\.\d{3}/g, '');
  const date = amzDate.slice(0, 8);
  const payloadSha256 = hexSha256(body);
  const headers: Record<string, string> = {
    host: url.host,
    'x-amz-content-sha256': payloadSha256,
    'x-amz-date': amzDate,
    ...Object.fromEntries(
      Object.entries(extraHeaders).map(([name, value]) => [
        name.toLowerCase(),
        value.trim().replace(/\s+/g, ' '),
      ]),
    ),
  };
  const names = Object.keys(headers).sort();
  const canonicalHeaders = names
    .map((name) => `${name}:${headers[name]}\n`)
    .join('');
  const signedHeaders = names.join(';');
  const canonicalRequest = [
    method,
    url.pathname,
    '',
    canonicalHeaders,
    signedHeaders,
    payloadSha256,
  ].join('\n');
  const scope = `${date}/auto/s3/aws4_request`;
  const stringToSign = [
    'AWS4-HMAC-SHA256',
    amzDate,
    scope,
    hexSha256(canonicalRequest),
  ].join('\n');
  const dateKey = hmac(`AWS4${credentials.secretAccessKey}`, date);
  const regionKey = hmac(dateKey, 'auto');
  const serviceKey = hmac(regionKey, 's3');
  const signingKey = hmac(serviceKey, 'aws4_request');
  const signature = createHmac('sha256', signingKey)
    .update(stringToSign)
    .digest('hex');
  const authorization =
    `AWS4-HMAC-SHA256 Credential=${credentials.accessKeyId}/${scope}, ` +
    `SignedHeaders=${signedHeaders}, Signature=${signature}`;
  const requestHeaders = new Headers(extraHeaders);
  requestHeaders.set('x-amz-content-sha256', payloadSha256);
  requestHeaders.set('x-amz-date', amzDate);
  requestHeaders.set('authorization', authorization);
  return {
    url,
    init: {
      method,
      headers: requestHeaders,
      ...(method === 'PUT'
        ? { body: Uint8Array.from(body).buffer }
        : {}),
    },
  };
}
