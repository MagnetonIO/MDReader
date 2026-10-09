/** Public transport only. Native code remains responsible for signatures, scope and freshness. */
export interface PublicPairPacket {
  kind: 'mdreader-pair-request' | 'mdreader-pair-approval';
  account: string; libraryId: string; pairId: string;
  fingerprints: { label: string; value: string }[];
}
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const thumb = /^[A-Za-z0-9_-]{43}$/;
const digest = /^[0-9a-f]{64}$/;
function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Choose an MDReader public device request or approval file.');
  return value as Record<string, unknown>;
}
function fields(value: Record<string, unknown>, allowed: string[]) {
  if (Object.keys(value).length !== allowed.length || Object.keys(value).some(key => !allowed.includes(key))) throw new Error('This file is not a public device packet. Recovery files and secrets belong in the native recovery dialog.');
}
function publicKey(value: unknown) {
  const key = object(value); fields(key, ['kty', 'crv', 'x', 'y']);
  if (key.kty !== 'EC' || key.crv !== 'P-256' || typeof key.x !== 'string' || !thumb.test(key.x) || typeof key.y !== 'string' || !thumb.test(key.y)) throw new Error('The public device key format is invalid.');
}
export function parsePublicPacket(text: string): PublicPairPacket {
  if (!text || new TextEncoder().encode(text).length > 8192) throw new Error('Device files must contain at most 8 KiB of public JSON.');
  let value: Record<string, unknown>;
  try { value = object(JSON.parse(text)); } catch { throw new Error('Choose an MDReader public device request or approval JSON file.'); }
  if (value.v !== 1 || typeof value.account !== 'string' || !digest.test(value.account) || typeof value.libraryId !== 'string' || !uuid.test(value.libraryId) || typeof value.pairId !== 'string' || !uuid.test(value.pairId)) throw new Error('The device packet identity is invalid.');
  const fingerprints: PublicPairPacket['fingerprints'] = [];
  if (value.kind === 'mdreader-pair-request') {
    fields(value, ['v', 'kind', 'account', 'libraryId', 'pairId', 'signingJwk', 'wrappingJwk', 'signingThumb', 'wrappingThumb', 'nonce', 'expires']);
    publicKey(value.signingJwk); publicKey(value.wrappingJwk);
    if (typeof value.signingThumb !== 'string' || !thumb.test(value.signingThumb) || typeof value.wrappingThumb !== 'string' || !thumb.test(value.wrappingThumb) || typeof value.nonce !== 'string' || !/^[A-Za-z0-9_-]{43}$/.test(value.nonce) || typeof value.expires !== 'number' || !Number.isSafeInteger(value.expires) || value.expires <= 0) throw new Error('The public device request is invalid.');
    fingerprints.push({ label: 'Device signing fingerprint', value: value.signingThumb }, { label: 'Device wrapping fingerprint', value: value.wrappingThumb });
  } else if (value.kind === 'mdreader-pair-approval') {
    fields(value, ['v', 'kind', 'account', 'libraryId', 'pairId', 'senderJwk', 'senderThumb', 'grantHash', 'jweHash']); publicKey(value.senderJwk);
    if (typeof value.senderThumb !== 'string' || !thumb.test(value.senderThumb) || typeof value.grantHash !== 'string' || !digest.test(value.grantHash) || typeof value.jweHash !== 'string' || !digest.test(value.jweHash)) throw new Error('The public device approval is invalid.');
    fingerprints.push({ label: 'Approving device fingerprint', value: value.senderThumb }, { label: 'Approval fingerprint', value: value.grantHash }, { label: 'Encrypted transfer fingerprint', value: value.jweHash });
  } else throw new Error('Only public device requests and approvals can be shared here.');
  return { kind: value.kind, account: value.account, libraryId: value.libraryId, pairId: value.pairId, fingerprints };
}
export function downloadPublicPacket(text: string) {
  const packet = parsePublicPacket(text);
  const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
  const link = document.createElement('a'); link.href = url; link.download = `${packet.kind}.json`; link.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
