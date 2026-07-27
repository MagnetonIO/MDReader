type Mapping = Record<string, unknown>;

const CHECKOUT =
  'actions/checkout@11d5960a326750d5838078e36cf38b85af677262';
const UPLOAD =
  'actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02';
const DOWNLOAD =
  'actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093';
const SETUP_NODE =
  'actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020';
const SYFT =
  'anchore/sbom-action/download-syft@f8bdd1d8ac5e901a77a92f111440fdb1b593736b';

const STEP_IDS: Record<string, string[]> = {
  prepare: [
    'checkout-release-source',
    'authorize-release-source',
    'seal-release-support',
    'upload-release-support',
    'setup-node-prepare',
    'enable-corepack-prepare',
    'install-dependencies-prepare',
    'authorize-publisher-bundle',
    'metadata',
    'validate-workflow-contract',
  ],
  'build-unsigned': [
    'checkout-build-source',
    'setup-node-build',
    'enable-corepack-build',
    'install-linux-compilation',
    'install-dependencies-build',
    'install-rust-targets',
    'generate-unsigned-release-config',
    'attest-unsigned-build',
    'prepare-direct-release',
    'verify-unsigned-build',
    'seal-unsigned-handoff',
    'upload-unsigned-handoff',
  ],
  'bundle-sign': [
    'setup-node-bundle',
    'download-unsigned-handoff',
    'download-release-support',
    'establish-tauri-runtime',
    'windows-admin-preflight',
    'verify-credential-boundaries',
    'require-release-secrets-macos',
    'require-release-secrets-windows',
    'require-release-secrets-linux',
    'inspect-windows-cert',
    'generate-bundle-config-macos',
    'generate-bundle-config-windows',
    'generate-bundle-config-linux',
    'verify-credentialed-release-config',
    'import-macos-cert',
    'import-windows-cert',
    'bundle-direct-release-macos',
    'bundle-direct-release-windows',
    'bundle-direct-release-linux',
    'cleanup-macos-cert',
    'cleanup-windows-cert',
    'seal-credentialed-output',
    'upload-credentialed-output',
  ],
  verify: [
    'checkout-verify-source',
    'setup-node-verify',
    'enable-corepack-verify',
    'install-dependencies-verify',
    'download-credentialed-output',
    'verify-credentialed-output',
    'macos-verify-signature',
    'macos-verify-notarization',
    'windows-verify-signature',
    'linux-verify-packages',
    'install-syft',
    'stage-release',
    'generate-sbom',
    'validate-staged-release',
    'upload-staged-release',
  ],
  aggregate: [
    'checkout-aggregate-source',
    'setup-node-aggregate',
    'enable-corepack-aggregate',
    'install-dependencies-aggregate',
    'download-staged-release-macos',
    'download-staged-release-windows',
    'download-staged-release-linux',
    'assemble-manifest',
    'validate-release',
    'seal-publication',
    'bundle-publisher',
    'verify-publisher-authority',
    'upload-publish-plan',
  ],
  publish: ['download-publish-plan', 'publish-r2'],
};

const CONDITIONS: Record<string, string> = {
  'build-unsigned/install-linux-compilation': "matrix.platform == 'linux'",
  'bundle-sign/windows-admin-preflight': "matrix.platform == 'windows'",
  'bundle-sign/require-release-secrets-macos': "matrix.platform == 'macos'",
  'bundle-sign/require-release-secrets-windows':
    "matrix.platform == 'windows'",
  'bundle-sign/require-release-secrets-linux': "matrix.platform == 'linux'",
  'bundle-sign/inspect-windows-cert': "matrix.platform == 'windows'",
  'bundle-sign/generate-bundle-config-macos': "matrix.platform == 'macos'",
  'bundle-sign/generate-bundle-config-windows':
    "matrix.platform == 'windows'",
  'bundle-sign/generate-bundle-config-linux': "matrix.platform == 'linux'",
  'bundle-sign/import-macos-cert': "matrix.platform == 'macos'",
  'bundle-sign/import-windows-cert': "matrix.platform == 'windows'",
  'bundle-sign/bundle-direct-release-macos': "matrix.platform == 'macos'",
  'bundle-sign/bundle-direct-release-windows':
    "matrix.platform == 'windows'",
  'bundle-sign/bundle-direct-release-linux': "matrix.platform == 'linux'",
  'bundle-sign/cleanup-macos-cert':
    "always() && matrix.platform == 'macos'",
  'bundle-sign/cleanup-windows-cert':
    "always() && matrix.platform == 'windows'",
  'verify/macos-verify-signature': "matrix.platform == 'macos'",
  'verify/macos-verify-notarization': "matrix.platform == 'macos'",
  'verify/windows-verify-signature': "matrix.platform == 'windows'",
  'verify/linux-verify-packages': "matrix.platform == 'linux'",
};

const SHELLS: Record<string, string> = {
  'build-unsigned/verify-unsigned-build': 'bash',
  'bundle-sign/windows-admin-preflight': 'pwsh',
  'bundle-sign/verify-credential-boundaries': 'bash',
  'bundle-sign/inspect-windows-cert': 'pwsh',
  'bundle-sign/import-windows-cert': 'pwsh',
  'bundle-sign/bundle-direct-release-windows': 'pwsh',
  'bundle-sign/cleanup-windows-cert': 'pwsh',
  'verify/windows-verify-signature': 'pwsh',
};

const ACTIONS: Record<string, { uses: string; with: Mapping }> = {
  'prepare/checkout-release-source': {
    uses: CHECKOUT,
    with: { 'persist-credentials': false, 'fetch-depth': 0 },
  },
  'prepare/upload-release-support': {
    uses: UPLOAD,
    with: {
      name: 'mdreader-release-support-${{ steps.seal-release-support.outputs.support_sha256 }}',
      path: '${{ runner.temp }}/mdreader-release-support',
      'if-no-files-found': 'error',
      'retention-days': 1,
    },
  },
  'prepare/setup-node-prepare': {
    uses: SETUP_NODE,
    with: { 'node-version': '22.13.1', cache: 'pnpm' },
  },
  'build-unsigned/checkout-build-source': {
    uses: CHECKOUT,
    with: { 'persist-credentials': false },
  },
  'build-unsigned/setup-node-build': {
    uses: SETUP_NODE,
    with: { 'node-version': '22.13.1', cache: 'pnpm' },
  },
  'build-unsigned/upload-unsigned-handoff': {
    uses: UPLOAD,
    with: {
      name: 'mdreader-unsigned-${{ matrix.platform }}-${{ steps.seal-unsigned-handoff.outputs.handoff_sha256 }}',
      path: 'target/release-unsigned',
      'if-no-files-found': 'error',
      'retention-days': 1,
    },
  },
  'bundle-sign/setup-node-bundle': {
    uses: SETUP_NODE,
    with: { 'node-version': '22.13.1' },
  },
  'bundle-sign/download-unsigned-handoff': {
    uses: DOWNLOAD,
    with: {
      pattern: 'mdreader-unsigned-${{ matrix.platform }}-*',
      path: '${{ runner.temp }}/mdreader-unsigned-download',
      'merge-multiple': false,
    },
  },
  'bundle-sign/download-release-support': {
    uses: DOWNLOAD,
    with: {
      name: 'mdreader-release-support-${{ needs.prepare.outputs.support_sha256 }}',
      path: '${{ runner.temp }}/mdreader-support-download/mdreader-release-support-${{ needs.prepare.outputs.support_sha256 }}',
    },
  },
  'bundle-sign/upload-credentialed-output': {
    uses: UPLOAD,
    with: {
      name: 'mdreader-bundled-${{ matrix.platform }}-${{ steps.seal-credentialed-output.outputs.archive_sha256 }}',
      path: '${{ runner.temp }}/mdreader-credentialed-archive',
      'if-no-files-found': 'error',
      'retention-days': 1,
    },
  },
  'verify/checkout-verify-source': {
    uses: CHECKOUT,
    with: { 'persist-credentials': false },
  },
  'verify/setup-node-verify': {
    uses: SETUP_NODE,
    with: { 'node-version': '22.13.1', cache: 'pnpm' },
  },
  'verify/download-credentialed-output': {
    uses: DOWNLOAD,
    with: {
      pattern: 'mdreader-bundled-${{ matrix.platform }}-*',
      path: '${{ runner.temp }}/mdreader-credentialed-download',
      'merge-multiple': false,
    },
  },
  'verify/install-syft': {
    uses: SYFT,
    with: { 'syft-version': 'v1.29.0' },
  },
  'verify/upload-staged-release': {
    uses: UPLOAD,
    with: {
      name: 'mdreader-release-${{ matrix.platform }}',
      path: 'target/release-stage',
      'if-no-files-found': 'error',
      'retention-days': 7,
    },
  },
  'aggregate/checkout-aggregate-source': {
    uses: CHECKOUT,
    with: { 'persist-credentials': false },
  },
  'aggregate/setup-node-aggregate': {
    uses: SETUP_NODE,
    with: { 'node-version': '22.13.1', cache: 'pnpm' },
  },
  ...Object.fromEntries(
    ['macos', 'windows', 'linux'].map((platform) => [
      `aggregate/download-staged-release-${platform}`,
      {
        uses: DOWNLOAD,
        with: {
          name: `mdreader-release-${platform}`,
          path: 'target/release-aggregate',
        },
      },
    ]),
  ),
  'aggregate/upload-publish-plan': {
    uses: UPLOAD,
    with: {
      name: 'mdreader-sealed-publication-${{ needs.prepare.outputs.source_commit }}',
      path: 'target/release-publish',
      'if-no-files-found': 'error',
      'retention-days': 1,
    },
  },
  'publish/download-publish-plan': {
    uses: DOWNLOAD,
    with: {
      name: 'mdreader-sealed-publication-${{ needs.prepare.outputs.source_commit }}',
      path: 'target/release-publish',
    },
  },
};

const BUILD_MATRIX = [
  {
    platform: 'macos',
    os: 'macos-14',
    target: 'universal-apple-darwin',
    bundles: 'dmg',
    binary: 'mdreader-app',
  },
  {
    platform: 'windows',
    os: 'windows-2025',
    target: 'x86_64-pc-windows-msvc',
    bundles: 'nsis',
    binary: 'mdreader-app.exe',
  },
  {
    platform: 'linux',
    os: 'ubuntu-24.04',
    target: 'x86_64-unknown-linux-gnu',
    bundles: 'appimage,deb,rpm',
    binary: 'mdreader-app',
  },
];

const BUNDLE_MATRIX = [
  {
    ...BUILD_MATRIX[0],
    environment: 'mdreader-production-macos',
    native_package: '@tauri-apps/cli-darwin-x64',
    native_integrity:
      'sha512-uFsGQAAfuyz1k/yGLmkWfkBlgKAqZfxqlHmLWx81QU27RJWfmbNHCIq8T8w1e+VClleIuZUjpHWfoE4E3DLo3A==',
  },
  {
    ...BUILD_MATRIX[1],
    environment: 'mdreader-production-windows',
    native_package: '@tauri-apps/cli-win32-x64-msvc',
    native_integrity:
      'sha512-+vDiqBIU5dMISg/wNvX3sF+ZHfgJGJ5T0AcO+EHNXV9GGAG+P5fzodlDXD3QdKCRgZxMoCm5PPvj3BqLNjBthw==',
  },
  {
    ...BUILD_MATRIX[2],
    environment: 'mdreader-production-linux',
    native_package: '@tauri-apps/cli-linux-x64-gnu',
    native_integrity:
      'sha512-2VRNWl84FOH0m2giiDkO2h0QXlcMJeX+zJDpI5kDIQAx6s+geF3v48F4DXfJez4GS/FdoDGnPnw1C2iYGbQ7bQ==',
  },
];

const VERIFY_MATRIX = BUILD_MATRIX.map(({ binary: _binary, ...entry }) => entry);

const SECRET_STEPS = new Set([
  'bundle-sign/require-release-secrets-macos',
  'bundle-sign/require-release-secrets-windows',
  'bundle-sign/require-release-secrets-linux',
  'bundle-sign/inspect-windows-cert',
  'bundle-sign/generate-bundle-config-macos',
  'bundle-sign/import-macos-cert',
  'bundle-sign/import-windows-cert',
  'bundle-sign/bundle-direct-release-macos',
  'bundle-sign/bundle-direct-release-windows',
  'bundle-sign/bundle-direct-release-linux',
  'publish/publish-r2',
]);

const BUNDLE_SECRET_ENVS: Record<string, Mapping> = {
  'require-release-secrets-macos': {
    TAURI_SIGNING_PRIVATE_KEY:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY_PASSWORD }}',
    APPLE_CERTIFICATE: '${{ secrets.MDREADER_APPLE_CERTIFICATE }}',
    APPLE_CERTIFICATE_PASSWORD:
      '${{ secrets.MDREADER_APPLE_CERTIFICATE_PASSWORD }}',
    APPLE_SIGNING_IDENTITY:
      '${{ secrets.MDREADER_APPLE_SIGNING_IDENTITY }}',
    APPLE_ID: '${{ secrets.MDREADER_APPLE_ID }}',
    APPLE_PASSWORD: '${{ secrets.MDREADER_APPLE_PASSWORD }}',
    APPLE_TEAM_ID: '${{ secrets.MDREADER_APPLE_TEAM_ID }}',
  },
  'require-release-secrets-windows': {
    TAURI_SIGNING_PRIVATE_KEY:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY_PASSWORD }}',
    WINDOWS_CERTIFICATE: '${{ secrets.MDREADER_WINDOWS_CERTIFICATE }}',
    WINDOWS_CERTIFICATE_PASSWORD:
      '${{ secrets.MDREADER_WINDOWS_CERTIFICATE_PASSWORD }}',
  },
  'require-release-secrets-linux': {
    TAURI_SIGNING_PRIVATE_KEY:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY_PASSWORD }}',
  },
  'inspect-windows-cert': {
    WINDOWS_CERTIFICATE: '${{ secrets.MDREADER_WINDOWS_CERTIFICATE }}',
    WINDOWS_CERTIFICATE_PASSWORD:
      '${{ secrets.MDREADER_WINDOWS_CERTIFICATE_PASSWORD }}',
  },
  'generate-bundle-config-macos': {
    APPLE_SIGNING_IDENTITY:
      '${{ secrets.MDREADER_APPLE_SIGNING_IDENTITY }}',
    MDREADER_UPDATER_PUBLIC_KEY: '${{ vars.MDREADER_UPDATER_PUBLIC_KEY }}',
    RELEASE_VERSION: '${{ needs.prepare.outputs.version }}',
  },
  'import-macos-cert': {
    APPLE_CERTIFICATE: '${{ secrets.MDREADER_APPLE_CERTIFICATE }}',
    APPLE_CERTIFICATE_PASSWORD:
      '${{ secrets.MDREADER_APPLE_CERTIFICATE_PASSWORD }}',
  },
  'import-windows-cert': {
    WINDOWS_CERTIFICATE: '${{ secrets.MDREADER_WINDOWS_CERTIFICATE }}',
    WINDOWS_CERTIFICATE_PASSWORD:
      '${{ secrets.MDREADER_WINDOWS_CERTIFICATE_PASSWORD }}',
    WINDOWS_CERT_THUMBPRINT:
      '${{ steps.inspect-windows-cert.outputs.thumbprint }}',
  },
  'bundle-direct-release-macos': {
    TAURI_SIGNING_PRIVATE_KEY:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY_PASSWORD }}',
    APPLE_ID: '${{ secrets.MDREADER_APPLE_ID }}',
    APPLE_PASSWORD: '${{ secrets.MDREADER_APPLE_PASSWORD }}',
    APPLE_TEAM_ID: '${{ secrets.MDREADER_APPLE_TEAM_ID }}',
    NODE_EXECUTABLE:
      '${{ steps.verify-credential-boundaries.outputs.node_path }}',
    TAURI_ENTRYPOINT:
      '${{ steps.verify-credential-boundaries.outputs.tauri_entrypoint }}',
    BUNDLE_WORKSPACE:
      '${{ steps.verify-credential-boundaries.outputs.bundle_workspace }}',
  },
  'bundle-direct-release-windows': {
    TAURI_SIGNING_PRIVATE_KEY:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY_PASSWORD }}',
    WINDOWS_CERT_STORE:
      '${{ steps.import-windows-cert.outputs.store_name }}',
    WINDOWS_CERT_STORE_OWNED:
      '${{ steps.import-windows-cert.outputs.store_owned }}',
    WINDOWS_CERT_THUMBPRINT:
      '${{ steps.inspect-windows-cert.outputs.thumbprint }}',
    NODE_EXECUTABLE:
      '${{ steps.verify-credential-boundaries.outputs.node_path }}',
    TAURI_ENTRYPOINT:
      '${{ steps.verify-credential-boundaries.outputs.tauri_entrypoint }}',
    BUNDLE_WORKSPACE:
      '${{ steps.verify-credential-boundaries.outputs.bundle_workspace }}',
  },
  'bundle-direct-release-linux': {
    TAURI_SIGNING_PRIVATE_KEY:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
      '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY_PASSWORD }}',
    NODE_EXECUTABLE:
      '${{ steps.verify-credential-boundaries.outputs.node_path }}',
    TAURI_ENTRYPOINT:
      '${{ steps.verify-credential-boundaries.outputs.tauri_entrypoint }}',
    BUNDLE_WORKSPACE:
      '${{ steps.verify-credential-boundaries.outputs.bundle_workspace }}',
  },
};

function mapping(value: unknown, path: string): Mapping {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${path} must be a mapping`);
  }
  return value as Mapping;
}

function list(value: unknown, path: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`${path} must be a list`);
  return value;
}

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value as Mapping)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, nested]) => [key, canonical(nested)]),
    );
  }
  return value;
}

function same(value: unknown, expected: unknown, message: string): void {
  if (JSON.stringify(canonical(value)) !== JSON.stringify(canonical(expected))) {
    throw new Error(message);
  }
}

function containsSecrets(value: unknown): boolean {
  if (typeof value === 'string') {
    return /\$\{\{[\s\S]*?\bsecrets\b[\s\S]*?\}\}/i.test(value);
  }
  if (Array.isArray(value)) return value.some(containsSecrets);
  if (value !== null && typeof value === 'object') {
    return Object.entries(value).some(
      ([key, nested]) => containsSecrets(key) || containsSecrets(nested),
    );
  }
  return false;
}

function stepMap(job: Mapping, jobName: string): Map<string, Mapping> {
  const entries = list(job.steps, `jobs.${jobName}.steps`).map((entry) =>
    mapping(entry, `jobs.${jobName}.steps[]`),
  );
  const actual = entries.map((step) => String(step.id ?? '<missing-id>'));
  same(
    actual,
    STEP_IDS[jobName],
    `${jobName} must retain its complete ordered step set`,
  );
  return new Map(entries.map((step) => [String(step.id), step]));
}

function requireRun(
  steps: Map<string, Mapping>,
  id: string,
  markers: string[],
  message: string,
): string {
  const run = steps.get(id)?.run;
  if (
    typeof run !== 'string' ||
    markers.some((marker) => !run.includes(marker))
  ) {
    throw new Error(message);
  }
  return run;
}

function validateAction(
  path: string,
  step: Mapping,
  expected: { uses: string; with: Mapping },
): void {
  if (
    step.uses !== expected.uses ||
    step.run !== undefined ||
    JSON.stringify(canonical(step.with)) !==
      JSON.stringify(canonical(expected.with))
  ) {
    throw new Error(`${path} does not use the exact immutable reviewed action`);
  }
}

function validateJobMetadata(jobs: Mapping): void {
  same(Object.keys(jobs), Object.keys(STEP_IDS), 'release workflow contains unreviewed top-level release jobs');
  const expected: Record<string, Mapping> = {
    prepare: {
      keys: ['runs-on', 'outputs', 'steps'],
      runs: 'ubuntu-24.04',
      needs: undefined,
    },
    'build-unsigned': {
      keys: ['needs', 'strategy', 'runs-on', 'steps'],
      runs: '${{ matrix.os }}',
      needs: 'prepare',
    },
    'bundle-sign': {
      keys: ['needs', 'environment', 'strategy', 'runs-on', 'steps'],
      runs: '${{ matrix.os }}',
      needs: ['prepare', 'build-unsigned'],
      environment: '${{ matrix.environment }}',
    },
    verify: {
      keys: ['needs', 'strategy', 'runs-on', 'steps'],
      runs: '${{ matrix.os }}',
      needs: ['prepare', 'bundle-sign'],
    },
    aggregate: {
      keys: ['needs', 'runs-on', 'outputs', 'steps'],
      runs: 'ubuntu-24.04',
      needs: ['prepare', 'verify'],
    },
    publish: {
      keys: ['needs', 'if', 'environment', 'runs-on', 'steps'],
      runs: 'ubuntu-24.04',
      needs: ['prepare', 'aggregate'],
      environment: 'mdreader-production-publication',
    },
  };
  for (const [name, spec] of Object.entries(expected)) {
    const job = mapping(jobs[name], `jobs.${name}`);
    same(Object.keys(job), spec.keys, `${name} job metadata is not exact`);
    same(job['runs-on'], spec.runs, `${name} runner is not exact`);
    same(job.needs, spec.needs, `${name} dependency boundary is invalid`);
    same(job.environment, spec.environment, `${name} protected environment is invalid`);
  }
  if (
    mapping(jobs.publish, 'jobs.publish').if !==
    "needs.prepare.outputs.publish_requested == 'true'"
  ) {
    throw new Error('publication condition is not exact');
  }
}

function validateStepMetadata(
  allSteps: Map<string, Map<string, Mapping>>,
): void {
  for (const [jobName, entries] of allSteps) {
    for (const [id, step] of entries) {
      const path = `${jobName}/${id}`;
      const allowed = ['name', 'id', 'uses', 'with', 'run', 'env', 'if', 'shell'];
      if (Object.keys(step).some((key) => !allowed.includes(key))) {
        throw new Error(`${path} contains unreviewed step metadata`);
      }
      if (step.if !== CONDITIONS[path]) {
        throw new Error(`${path} has an invalid condition`);
      }
      if (step.shell !== SHELLS[path]) {
        throw new Error(`${path} has an invalid shell`);
      }
      if (containsSecrets(step) !== SECRET_STEPS.has(path)) {
        throw new Error(`${path} violates the global secret allowlist`);
      }
      if (step.uses !== undefined) {
        const expected = ACTIONS[path];
        if (!expected) throw new Error(`${path} uses an unreviewed action`);
        validateAction(path, step, expected);
      } else if (ACTIONS[path]) {
        throw new Error(`${path} replaced a reviewed immutable action`);
      }
    }
  }
}

function validateMatrices(jobs: Mapping): void {
  const matrix = (name: string) =>
    mapping(
      mapping(mapping(jobs[name], `jobs.${name}`).strategy, `${name}.strategy`)
        .matrix,
      `${name}.matrix`,
    );
  for (const [name, include] of [
    ['build-unsigned', BUILD_MATRIX],
    ['bundle-sign', BUNDLE_MATRIX],
    ['verify', VERIFY_MATRIX],
  ] as const) {
    const strategy = mapping(
      mapping(jobs[name], `jobs.${name}`).strategy,
      `${name}.strategy`,
    );
    if (strategy['fail-fast'] !== false) {
      throw new Error(`${name} release matrix must be fail-closed`);
    }
    same(matrix(name).include, include, `${name} has an invalid exact release matrix`);
  }
}

function validatePrepare(
  job: Mapping,
  steps: Map<string, Mapping>,
): void {
  same(job.outputs, {
    version: '${{ steps.metadata.outputs.version }}',
    source_commit: '${{ steps.metadata.outputs.source_commit }}',
    published_at: '${{ steps.metadata.outputs.published_at }}',
    publish_requested: '${{ steps.metadata.outputs.publish_requested }}',
    publisher_sha256:
      '${{ steps.authorize-publisher-bundle.outputs.publisher_sha256 }}',
    support_sha256:
      '${{ steps.seal-release-support.outputs.support_sha256 }}',
    updater_public_key_sha256:
      '${{ steps.seal-release-support.outputs.updater_public_key_sha256 }}',
  }, 'prepare outputs are not bound to the reviewed authorizers');
  requireRun(
    steps,
    'authorize-release-source',
    ['refs/remotes/origin/main', 'workflow_dispatch', 'refs/heads/main'],
    'release source authorization is incomplete',
  );
  requireRun(
    steps,
    'seal-release-support',
    [
      'sign-windows-from-store.ps1',
      'windows-minimum.nsh',
      'updater_public_key_sha256',
      'sourceCommit: process.env.GITHUB_SHA',
    ],
    'reviewed support and updater identity are not sealed before package code',
  );
  requireRun(
    steps,
    'authorize-publisher-bundle',
    ['build-sealed-publisher.sh'],
    'publisher source is not independently authorized',
  );
}

function validateUnsigned(
  job: Mapping,
  steps: Map<string, Mapping>,
): void {
  if (job.environment !== undefined || containsSecrets(job)) {
    throw new Error('uncredentialed build must not resolve a protected environment or secret');
  }
  requireRun(
    steps,
    'generate-unsigned-release-config',
    [
      'ACTUAL_KEY_SHA256',
      'EXPECTED_UPDATER_PUBLIC_KEY_SHA256',
      'generate-direct-release-config.sh linux',
    ],
    'unsigned build config does not bind the sealed updater identity',
  );
  requireRun(
    steps,
    'verify-unsigned-build',
    [
      'EXPECTED_BUILD_ATTESTATION_SHA256',
      'EXPECTED_RELEASE_PLATFORM',
      'releaseConfigSha256',
      'config.bundle?.windows !== undefined',
      'config.bundle?.macOS !== undefined',
    ],
    'unsigned build attestation is incomplete',
  );
  requireRun(
    steps,
    'seal-unsigned-handoff',
    [
      'const sources = [',
      "'32x32.png'",
      'process.env.RELEASE_BINARY',
      'handoff_sha256=',
    ],
    'unsigned binary and resource handoff is not closed and hash-addressed',
  );
}

function validateBundle(
  job: Mapping,
  steps: Map<string, Mapping>,
): void {
  const entries = [...steps.values()];
  for (const [id, expected] of Object.entries(BUNDLE_SECRET_ENVS)) {
    const credentialStep = steps.get(id)!;
    same(
      credentialStep.env,
      expected,
      `${id} has an unreviewed signing environment`,
    );
    if (
      containsSecrets(
        Object.fromEntries(
          Object.entries(credentialStep).filter(([key]) => key !== 'env'),
        ),
      )
    ) {
      throw new Error(`${id} accesses a secret outside its reviewed environment`);
    }
  }
  if (
    entries.some(
      (step) =>
        typeof step.uses === 'string' &&
        step.uses.startsWith('actions/checkout@'),
    )
  ) {
    throw new Error('credentialed bundle job must not checkout application source');
  }
  const boundaryIndex = STEP_IDS['bundle-sign']!.indexOf(
    'verify-credential-boundaries',
  );
  for (const [index, step] of entries.entries()) {
    if (index <= boundaryIndex && containsSecrets(step)) {
      throw new Error('credential or secret was resolved before the closed handoff boundary');
    }
    const run = String(step.run ?? '');
    if (
      /scripts\/release\/(?:bundle|prepare)-direct-release\.sh/.test(run) ||
      /(?:^|[^\w])(?:pnpm|corepack)\s/.test(run) ||
      /node\s+(?:\.\/)?node_modules\/@tauri-apps/.test(run) ||
      /\btauri\s+build\b/.test(run)
    ) {
      throw new Error('credentialed job can execute checkout-controlled build code');
    }
  }
  requireRun(
    steps,
    'establish-tauri-runtime',
    [
      'npm pack --ignore-scripts --json',
      'TAURI_CLI_INTEGRITY',
      'TAURI_NATIVE_INTEGRITY',
      'isolated Tauri runtime contains a symbolic link',
      'Node runtime is not an exact regular executable',
      'sha256: hash(nodeBody)',
      'runtime_manifest_sha256=',
    ],
    'isolated Tauri and Node runtime is not fully inventoried before credentials',
  );
  const boundary = requireRun(
    steps,
    'verify-credential-boundaries',
    [
      'EXPECTED_RUNTIME_MANIFEST_SHA256',
      'unexpected handoff path',
      'Node runtime inventory mismatch',
      "join(tauriRoot, 'Cargo.toml')",
      '[features]\\ndirect-updater = []',
      "join(tauriRoot, 'tauri.conf.json')",
      'tauri_entrypoint=',
      'bundle_workspace=',
    ],
    'credential boundary does not verify the closed handoff, runtime, and minimal scaffold',
  );
  if (
    boundary.includes('github.workspace') ||
    boundary.includes('GITHUB_WORKSPACE')
  ) {
    throw new Error('credential boundary can fall back to checkout-controlled paths');
  }
  requireRun(
    steps,
    'verify-credentialed-release-config',
    [
      "macos: ['universal-apple-darwin', 'dmg']",
      "windows: ['x86_64-pc-windows-msvc', 'nsis']",
      "linux: ['x86_64-unknown-linux-gnu', 'appimage,deb,rpm']",
      'windows?.nsis?.installerHooks',
      'release_config_sha256=',
    ],
    'credentialed release config lacks the exact platform tuple or Windows NSIS hook',
  );
  for (const id of [
    'bundle-direct-release-macos',
    'bundle-direct-release-linux',
  ]) {
    const run = requireRun(
      steps,
      id,
      [
        'NODE_EXECUTABLE',
        'TAURI_ENTRYPOINT',
        'exec "$NODE_EXECUTABLE" "$TAURI_ENTRYPOINT" bundle',
        '--features direct-updater',
        '--config "$RUNNER_TEMP/mdreader-release-config.json"',
      ],
      `${id} does not invoke only the isolated inventoried Tauri runtime`,
    );
    if (run.includes('node_modules/@tauri-apps')) {
      throw new Error(`${id} can execute checkout-controlled node_modules`);
    }
  }
  const windowsBundle = requireRun(
    steps,
    'bundle-direct-release-windows',
    [
      '$env:NODE_EXECUTABLE',
      '$env:TAURI_ENTRYPOINT',
      '& $env:NODE_EXECUTABLE $env:TAURI_ENTRYPOINT bundle',
      '--features direct-updater',
      "Join-Path $env:RUNNER_TEMP 'mdreader-release-config.json'",
      '$LASTEXITCODE',
    ],
    'bundle-direct-release-windows does not invoke only the isolated inventoried Tauri runtime',
  );
  if (windowsBundle.includes('node_modules/@tauri-apps')) {
    throw new Error(
      'bundle-direct-release-windows can execute checkout-controlled node_modules',
    );
  }
  requireRun(
    steps,
    'seal-credentialed-output',
    [
      'releaseConfigSha256',
      'credentialed-output.json',
      'credentialed output contains unsupported entry',
      'credentialed-output.tar',
      'archive_sha256=',
    ],
    'credentialed output is not completely inventoried',
  );
}

function validateVerify(
  job: Mapping,
  steps: Map<string, Mapping>,
): void {
  if (job.environment !== undefined || containsSecrets(job)) {
    throw new Error('verification job must be fresh and uncredentialed');
  }
  requireRun(
    steps,
    'verify-credentialed-output',
    [
      'scripts/release/verify-credentialed-output.sh',
      'mdreader-credentialed-download',
      'mdreader-credentialed-output',
      'needs.prepare.outputs.updater_public_key_sha256',
    ],
    'credentialed output is not verified before platform acceptance',
  );
  requireRun(
    steps,
    'macos-verify-signature',
    ['verify-macos-signature.sh', 'mdreader-credentialed-output/bundle'],
    'macOS signature verification is incomplete',
  );
  requireRun(
    steps,
    'macos-verify-notarization',
    ['verify-macos-notarization.sh', 'mdreader-credentialed-output/bundle'],
    'macOS notarization verification is incomplete',
  );
  requireRun(
    steps,
    'windows-verify-signature',
    ['verify-windows-signature.ps1', 'mdreader-credentialed-output/bundle'],
    'Windows signing verification is incomplete',
  );
  requireRun(
    steps,
    'linux-verify-packages',
    ['verify-linux-packages.sh', 'mdreader-credentialed-output/bundle'],
    'Linux package verification is incomplete',
  );
  requireRun(
    steps,
    'stage-release',
    [
      'stage-platform.sh',
      'credentialed-output/release-config.json',
      'credentialed-output/credentialed-output.json',
    ],
    'staging is not bound to the verified credentialed output and checksum metadata',
  );
}

function validateAggregateAndPublish(
  aggregate: Mapping,
  aggregateSteps: Map<string, Mapping>,
  publish: Mapping,
  publishSteps: Map<string, Mapping>,
): void {
  same(aggregate.outputs, {
    publish_plan_sha256:
      '${{ steps.seal-publication.outputs.publish_plan_sha256 }}',
  }, 'aggregate publisher output is not exact');
  requireRun(
    aggregateSteps,
    'assemble-manifest',
    ['release-cli.ts assemble'],
    'aggregate manifest assembly is missing',
  );
  requireRun(
    aggregateSteps,
    'validate-release',
    ['release-cli.ts validate'],
    'aggregate validation command is missing',
  );
  requireRun(
    aggregateSteps,
    'verify-publisher-authority',
    [
      'needs.prepare.outputs.publisher_sha256',
      'publisher bundle differs from the independently authorized source build',
    ],
    'publisher does not retain independent prepare-time authorization',
  );
  const publisher = publishSteps.get('publish-r2')!;
  same(publisher.env, {
    CLOUDFLARE_ACCOUNT_ID: '${{ secrets.CLOUDFLARE_ACCOUNT_ID }}',
    MDREADER_R2_ACCESS_KEY_ID:
      '${{ secrets.MDREADER_R2_ACCESS_KEY_ID }}',
    MDREADER_R2_SECRET_ACCESS_KEY:
      '${{ secrets.MDREADER_R2_SECRET_ACCESS_KEY }}',
    RELEASE_BUCKET: '${{ vars.MDREADER_RELEASE_BUCKET }}',
  }, 'publisher environment contains unreviewed publication credentials');
  requireRun(
    publishSteps,
    'publish-r2',
    [
      'needs.prepare.outputs.publisher_sha256',
      'needs.aggregate.outputs.publish_plan_sha256',
      'publish-sealed-plan.mjs',
    ],
    'publication is not bound to the independently authorized publisher and sealed plan',
  );
  if (containsSecrets(aggregate) || publish.env !== undefined) {
    throw new Error('publication credentials escaped the reviewed publish step');
  }
}

export function validateReleaseWorkflow(workflowValue: unknown): void {
  const workflow = mapping(workflowValue, 'workflow');
  same(
    Object.keys(workflow),
    ['name', 'on', 'permissions', 'concurrency', 'defaults', 'jobs'],
    'release workflow metadata is not exact',
  );
  same(workflow.permissions, { contents: 'read' }, 'release workflow permissions are not least privilege');
  same(workflow.concurrency, {
    group: 'mdreader-stable-release',
    'cancel-in-progress': false,
  }, 'stable-channel concurrency is not exact');
  same(workflow.defaults, { run: { shell: 'bash' } }, 'release workflow default shell is not exact');
  const jobs = mapping(workflow.jobs, 'jobs');
  validateJobMetadata(jobs);
  validateMatrices(jobs);
  const allSteps = new Map<string, Map<string, Mapping>>();
  for (const [name] of Object.entries(STEP_IDS)) {
    allSteps.set(name, stepMap(mapping(jobs[name], `jobs.${name}`), name));
  }
  validateStepMetadata(allSteps);
  validatePrepare(mapping(jobs.prepare, 'jobs.prepare'), allSteps.get('prepare')!);
  validateUnsigned(
    mapping(jobs['build-unsigned'], 'jobs.build-unsigned'),
    allSteps.get('build-unsigned')!,
  );
  validateBundle(
    mapping(jobs['bundle-sign'], 'jobs.bundle-sign'),
    allSteps.get('bundle-sign')!,
  );
  validateVerify(
    mapping(jobs.verify, 'jobs.verify'),
    allSteps.get('verify')!,
  );
  validateAggregateAndPublish(
    mapping(jobs.aggregate, 'jobs.aggregate'),
    allSteps.get('aggregate')!,
    mapping(jobs.publish, 'jobs.publish'),
    allSteps.get('publish')!,
  );
}
