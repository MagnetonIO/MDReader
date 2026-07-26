export interface ScannedFile {
  path: string;
  contents: string;
}

export interface SecretFinding {
  path: string;
  reason: string;
}

const PRIVATE_KEY_BLOCK = /-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/;
const LITERAL_RELEASE_CREDENTIAL =
  /\b(?:MDREADER_[A-Z0-9_]*(?:PRIVATE_KEY|CERTIFICATE|PASSWORD|API_TOKEN|ACCESS_KEY_ID|SECRET_ACCESS_KEY)|CLOUDFLARE_API_TOKEN)\s*[:=]\s*["']?[A-Za-z0-9+/=]{20,}["']?/;

export function findReleaseSecretMaterial(files: ScannedFile[]): SecretFinding[] {
  const findings: SecretFinding[] = [];
  for (const file of files) {
    if (PRIVATE_KEY_BLOCK.test(file.contents)) {
      findings.push({ path: file.path, reason: 'tracked private key block' });
    }
    if (LITERAL_RELEASE_CREDENTIAL.test(file.contents)) {
      findings.push({ path: file.path, reason: 'literal release credential assignment' });
    }
  }
  return findings;
}
