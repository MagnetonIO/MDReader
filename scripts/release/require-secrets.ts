const platform = process.argv[2];
const common = [
  'TAURI_SIGNING_PRIVATE_KEY',
  'TAURI_SIGNING_PRIVATE_KEY_PASSWORD',
  'MDREADER_UPDATER_PUBLIC_KEY',
];
const platformSecrets: Record<string, string[]> = {
  macos: [
    'APPLE_CERTIFICATE',
    'APPLE_CERTIFICATE_PASSWORD',
    'APPLE_SIGNING_IDENTITY',
    'APPLE_ID',
    'APPLE_PASSWORD',
    'APPLE_TEAM_ID',
  ],
  windows: ['WINDOWS_CERTIFICATE', 'WINDOWS_CERTIFICATE_PASSWORD'],
  linux: [],
};

if (!(platform in platformSecrets)) {
  throw new Error(`unsupported release platform ${platform ?? '(missing)'}`);
}
const missing = [...common, ...platformSecrets[platform]!].filter(
  (name) => !process.env[name],
);
if (missing.length > 0) {
  throw new Error(`missing required release credentials: ${missing.join(', ')}`);
}
process.stdout.write(`release credentials present for ${platform}\n`);
