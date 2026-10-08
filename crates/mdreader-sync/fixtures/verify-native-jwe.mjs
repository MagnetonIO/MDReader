// Synthetic fixture conformance only. Never invoke with real custody material.
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import assert from "node:assert/strict";
const web = process.env.MDREADER_SYNC_WEB_WORKTREE;
if (!web) throw new Error("Companion web worktree required");
const jose = await import(pathToFileURL(createRequire(join(web, "package.json")).resolve("jose")).href);
let input = "";
for await (const chunk of process.stdin) {
  input += chunk;
  if (input.length > 32768) throw new Error("Synthetic fixture bounds");
}
const { fixture, pair, recovery, secret } = JSON.parse(input);
assert.match(fixture.notice, /SYNTHETIC PUBLIC TEST KEYS/);
const key = await jose.importJWK(fixture.recipientPrivateJwk, "ECDH-ES+A256KW");
const paired = await jose.compactDecrypt(pair, key, { keyManagementAlgorithms: ["ECDH-ES+A256KW"], contentEncryptionAlgorithms: ["A256GCM"] });
assert.equal(new TextDecoder().decode(paired.plaintext), fixture.plaintextUtf8);
assert.equal(paired.protectedHeader.typ, "mdreader-pair+jwe");
assert.equal(paired.protectedHeader.apu, jose.base64url.encode(fixture.apuUtf8));
assert.equal(paired.protectedHeader.apv, jose.base64url.encode(fixture.apvUtf8));
const raw = jose.base64url.decode(secret);
const recovered = await jose.compactDecrypt(recovery, raw, { keyManagementAlgorithms: ["dir"], contentEncryptionAlgorithms: ["A256GCM"] });
assert.equal(new TextDecoder().decode(recovered.plaintext), fixture.plaintextUtf8);
assert.equal(recovered.protectedHeader.typ, "mdreader-recovery+jwe");
const back = await new jose.CompactEncrypt(new TextEncoder().encode(fixture.plaintextUtf8)).setProtectedHeader({ alg: "dir", enc: "A256GCM", typ: "mdreader-recovery+jwe" }).encrypt(raw);
process.stdout.write(back);
