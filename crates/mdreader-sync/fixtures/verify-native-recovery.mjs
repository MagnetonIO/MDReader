// Synthetic native/browser recovery payload conformance only. No production keys.
import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import assert from "node:assert/strict";
const web = process.env.MDREADER_SYNC_WEB_WORKTREE;
if (!web) throw new Error("Companion web worktree required");
const jose = await import(pathToFileURL(createRequire(join(web,"package.json")).resolve("jose")).href);
let input = "";
for await (const chunk of process.stdin) {
  input += chunk;
  if (input.length > 32768) throw new Error("Synthetic bounds");
}
const { notice, file, secret, grant } = JSON.parse(input);
assert.equal(notice,"SYNTHETIC PUBLIC TEST KEYS");
assert.equal(file.v,1); assert.equal(file.kind,"mdreader-recovery");
const raw = jose.base64url.decode(secret);
const decrypted = await jose.compactDecrypt(file.jwe,raw,{keyManagementAlgorithms:["dir"],contentEncryptionAlgorithms:["A256GCM"]});
assert.equal(decrypted.protectedHeader.typ,"mdreader-recovery+jwe");
const payload = JSON.parse(new TextDecoder().decode(decrypted.plaintext));
assert.equal(payload.v,1); assert.equal(payload.kind,"mdreader-library-keys");
const key = await jose.importJWK(payload.recoveryPrivateJwk,"ES256",{extractable:true});
const exported = await jose.exportJWK(key);
const { d, ...publicJwk } = exported;
const verified = await jose.jwtVerify(grant,await jose.importJWK(publicJwk,"ES256"),{algorithms:["ES256"],typ:"mdreader-recovery-grant+jwt"});
assert.equal(verified.payload.account,payload.account);
assert.equal(verified.payload.libraryId,payload.libraryId);
payload.recoveryPrivateJwk = { kty:exported.kty,crv:exported.crv,x:exported.x,y:exported.y,d };
const jwe = await new jose.CompactEncrypt(new TextEncoder().encode(JSON.stringify(payload))).setProtectedHeader({alg:"dir",enc:"A256GCM",typ:"mdreader-recovery+jwe"}).encrypt(raw);
process.stdout.write(JSON.stringify({v:1,kind:"mdreader-recovery",jwe}));
