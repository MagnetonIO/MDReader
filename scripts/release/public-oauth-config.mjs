import { createHash } from 'node:crypto';
import { appendFileSync } from 'node:fs';

const SHA256 = /^[0-9a-f]{64}$/;
const GOOGLE_CLIENT_ID =
  /^[0-9]+-[A-Za-z0-9_-]+\.apps\.googleusercontent\.com$/;
const DROPBOX_CLIENT_ID = /^[A-Za-z0-9_-]{8,128}$/;
const DROPBOX_REDIRECT =
  /^http:\/\/127\.0\.0\.1:([1-9][0-9]{0,4})\/oauth\/dropbox$/;

const inputs = [
  {
    name: 'MDREADER_GOOGLE_DRIVE_CLIENT_ID',
    expected: 'EXPECTED_GOOGLE_DRIVE_CLIENT_ID_SHA256',
    output: 'google_drive_client_id_sha256',
  },
  {
    name: 'MDREADER_DROPBOX_CLIENT_ID',
    expected: 'EXPECTED_DROPBOX_CLIENT_ID_SHA256',
    output: 'dropbox_client_id_sha256',
  },
  {
    name: 'MDREADER_DROPBOX_REDIRECT_URI',
    expected: 'EXPECTED_DROPBOX_REDIRECT_URI_SHA256',
    output: 'dropbox_redirect_uri_sha256',
  },
];

function required(name) {
  const value = process.env[name];
  if (typeof value !== 'string' || value.length === 0) {
    throw new Error(`public OAuth configuration is missing required ${name}`);
  }
  return value;
}

function sha256(value) {
  return createHash('sha256').update(value, 'utf8').digest('hex');
}

function validateGoogleClientId(value) {
  if (value.length > 512 || !GOOGLE_CLIENT_ID.test(value)) {
    throw new Error(
      'public OAuth configuration has an invalid MDREADER_GOOGLE_DRIVE_CLIENT_ID',
    );
  }
}

function validateDropboxClientId(value) {
  if (!DROPBOX_CLIENT_ID.test(value)) {
    throw new Error(
      'public OAuth configuration has an invalid MDREADER_DROPBOX_CLIENT_ID',
    );
  }
}

function validateDropboxRedirect(value) {
  const match = DROPBOX_REDIRECT.exec(value);
  if (!match || Number(match[1]) > 65_535) {
    throw new Error(
      'public OAuth configuration has an invalid MDREADER_DROPBOX_REDIRECT_URI',
    );
  }
}

function validatedHashes() {
  const googleClientId = required('MDREADER_GOOGLE_DRIVE_CLIENT_ID');
  const dropboxClientId = required('MDREADER_DROPBOX_CLIENT_ID');
  const dropboxRedirect = required('MDREADER_DROPBOX_REDIRECT_URI');
  validateGoogleClientId(googleClientId);
  validateDropboxClientId(dropboxClientId);
  validateDropboxRedirect(dropboxRedirect);
  return new Map([
    ['MDREADER_GOOGLE_DRIVE_CLIENT_ID', sha256(googleClientId)],
    ['MDREADER_DROPBOX_CLIENT_ID', sha256(dropboxClientId)],
    ['MDREADER_DROPBOX_REDIRECT_URI', sha256(dropboxRedirect)],
  ]);
}

const command = process.argv[2];
const hashes = validatedHashes();
if (command === 'emit-github-output') {
  const outputPath = required('GITHUB_OUTPUT');
  appendFileSync(
    outputPath,
    inputs
      .map(({ name, output }) => `${output}=${hashes.get(name)}`)
      .join('\n') + '\n',
  );
} else if (command === 'assert-hashes') {
  for (const { name, expected } of inputs) {
    const expectedHash = required(expected);
    if (!SHA256.test(expectedHash) || hashes.get(name) !== expectedHash) {
      throw new Error(
        `public OAuth configuration ${name} does not match its immutable release hash`,
      );
    }
  }
} else {
  throw new Error(
    'usage: public-oauth-config.mjs <emit-github-output|assert-hashes>',
  );
}
