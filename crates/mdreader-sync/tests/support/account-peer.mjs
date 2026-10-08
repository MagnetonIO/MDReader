// Public synthetic browser peer. Every enrollment/upload/commit enters the real
// Worker handler; this helper cannot write its SQLite tables or native custody.
import { createHash, createCipheriv, randomBytes, randomUUID } from 'node:crypto';
export function accountPeer({ runtime, jose, origin, issuer, tokens }) {
  const libraryId = '33333333-3333-4333-8333-333333333333';
  const keyId = '44444444-4444-4444-8444-444444444444';
  const documentId = '11111111-1111-4111-8111-111111111111';
  const contentRevision = '22222222-2222-4222-8222-222222222222';
  const key = Buffer.alloc(32); // Published test material, never native-generated keys.
  const source = Buffer.from('\ufeff# Public synthetic fixture\r\n\r\nExact Markdown: π.\r\n\r\n![Pixel](pixel.png)\r\n');
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8_x8AAwMCAO-ip1sAAAAASUVORK5CYII', 'base64url');
  const hash = (b, encoding = 'hex') => createHash('sha256').update(b).digest(encoding);
  const publicJwk = ({ kty, crv, x, y }) => ({ kty, crv, x, y });
  let ready, signing, signingJwk, recoveryFile, pinRevision, deleted = false;
  async function request(path, method, value, pair) {
    const body = value === undefined ? '' : typeof value === 'string' ? value : JSON.stringify(value);
    const { token, loginToken } = pair || await tokens();
    const candidate = path === '/v1/enrollment/bootstrap';
    const claims = { v: 2, htm: method, htu: origin() + path, ath: hash(token, 'base64url'), bht: hash(body, 'base64url'), ...(candidate ? { lth: hash(loginToken, 'base64url') } : {}) };
    const proof = await new jose.SignJWT(claims).setProtectedHeader({ alg: 'ES256', typ: 'mdreader-proof+jwt', jwk: signingJwk }).setIssuedAt().setJti(randomUUID()).sign(signing.privateKey);
    const response = await runtime.dispatchFetch(origin() + path, { method, headers: { authorization: `Bearer ${token}`, 'mdreader-proof': proof, ...(candidate ? { 'mdreader-login-token': loginToken } : {}) }, body: ['GET', 'HEAD'].includes(method) ? undefined : body });
    if (!response.ok) throw new Error(`synthetic peer request rejected (${response.status}, ${path.split('?')[0]})`);
    return response.json();
  }
  function encrypt(itemId, revisionId, value) {
    const nonce = randomBytes(12), cipher = createCipheriv('aes-256-gcm', key, nonce);
    cipher.setAAD(Buffer.from(JSON.stringify(['mdreader', 1, libraryId, itemId, revisionId, keyId])));
    const ciphertext = Buffer.concat([cipher.update(Buffer.from(JSON.stringify(value))), cipher.final(), cipher.getAuthTag()]);
    const envelope = JSON.stringify({ v: 1, alg: 'A256GCM', keyId, nonce: nonce.toString('base64url'), ciphertext: ciphertext.toString('base64url') });
    return { blob: hash(envelope), envelope };
  }
  async function upload(operationId, encrypted) {
    const receipt = await request(`/v1/blobs/${encrypted.blob}?operationId=${operationId}&libraryId=${libraryId}`, 'PUT', encrypted.envelope);
    if (receipt.v !== 2 || receipt.operationId !== operationId || receipt.libraryId !== libraryId || receipt.blob !== encrypted.blob || receipt.bytes !== Buffer.byteLength(encrypted.envelope) || receipt.device !== await jose.calculateJwkThumbprint(signingJwk) || receipt.account !== hash(JSON.stringify([issuer, 'synthetic-alice']))) throw new Error('synthetic scoped receipt mismatch');
  }
  async function publish(itemId, revisionId, payload, parents = []) {
    const operationId = randomUUID(), encrypted = payload === null ? null : encrypt(itemId, revisionId, payload);
    if (encrypted) await upload(operationId, encrypted);
    return request('/v1/commit', 'POST', { operationId, itemId, revisionId, parents, blob: encrypted?.blob ?? null, deleted: payload === null });
  }
  async function seed() {
    signing = await jose.generateKeyPair('ES256', { extractable: true });
    signingJwk = publicJwk(await jose.exportJWK(signing.publicKey));
    const wrapping = await jose.generateKeyPair('ECDH-ES+A256KW', { extractable: true });
    const recovery = await jose.generateKeyPair('ES256', { extractable: true });
    const wrappingJwk = publicJwk(await jose.exportJWK(wrapping.publicKey));
    const recoveryJwk = publicJwk(await jose.exportJWK(recovery.publicKey));
    const response = await runtime.dispatchFetch(origin() + '/v1/enrollment/challenge', { method: 'POST', body: JSON.stringify({ kind: 'bootstrap', libraryId, signingJwk, wrappingJwk, recoveryJwk }) });
    if (!response.ok) throw new Error('synthetic bootstrap challenge rejected');
    const challenge = await response.json();
    await request('/v1/enrollment/bootstrap', 'POST', { challenge: challenge.challenge, signingJwk, wrappingJwk, recoveryJwk }, await tokens(challenge.nonce));
    const privateKey = await jose.exportJWK(recovery.privateKey);
    const bundle = { v: 1, kind: 'mdreader-library-keys', account: hash(JSON.stringify([issuer, 'synthetic-alice'])), libraryId, keys: [{ keyId, raw: key.toString('base64url') }], recoveryPrivateJwk: { ...publicJwk(privateKey), d: privateKey.d } };
    const jwe = await new jose.CompactEncrypt(Buffer.from(JSON.stringify(bundle))).setProtectedHeader({ alg: 'dir', enc: 'A256GCM', typ: 'mdreader-recovery+jwe' }).encrypt(Buffer.alloc(32, 91));
    recoveryFile = JSON.stringify({ v: 1, kind: 'mdreader-recovery', jwe });
    const operationId = randomUUID();
    async function chunk(role, path, bytes) {
      const itemId = randomUUID(), revisionId = randomUUID();
      const sealed = encrypt(itemId, revisionId, { v: 1, kind: 'mdreader-chunk', documentId, revisionId: contentRevision, role, path, index: 0, count: 1, data: bytes.toString('base64url') });
      await upload(operationId, sealed);
      return { itemId, revisionId, blob: sealed.blob, bytes: bytes.length, sha256: hash(bytes) };
    }
    const sourceChunk = await chunk('markdown', null, source), assetChunk = await chunk('asset', 'pixel.png', png);
    const root = encrypt(documentId, contentRevision, { v: 1, kind: 'mdreader-content', documentId, revisionId: contentRevision, title: 'Public synthetic π.md', source: { bytes: source.length, sha256: hash(source), chunks: [sourceChunk] }, assets: [{ path: 'pixel.png', mime: 'image/png', bytes: png.length, sha256: hash(png), chunks: [assetChunk] }] });
    await upload(operationId, root);
    await request('/v1/commit', 'POST', { operationId, itemId: documentId, revisionId: contentRevision, parents: [], blob: root.blob, deleted: false });
  }
  return async ({ action }) => {
    ready ??= seed(); await ready;
    if (action === 'seed') return { libraryId, recoveryFile, sourceUtf8: source.toString(), assetSha256: hash(png) };
    if (action === 'pin' && !pinRevision && !deleted) {
      pinRevision = randomUUID();
      return publish('99999999-9999-4999-8999-999999999999', pinRevision, { v: 1, kind: 'mdreader-user-state', documentId, pinned: true, position: { kind: 'line', revisionId: contentRevision, line: 2 } });
    }
    if (action === 'delete' && !deleted) { deleted = true; return publish(documentId, randomUUID(), null, [contentRevision]); }
    throw new Error('synthetic peer action invalid or repeated');
  };
}
