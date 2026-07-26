type Mapping = Record<string, unknown>;

const CHECKOUT =
  'actions/checkout@11d5960a326750d5838078e36cf38b85af677262';
const UPLOAD =
  'actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02';
const DOWNLOAD =
  'actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093';
const SYFT =
  'anchore/sbom-action/download-syft@f8bdd1d8ac5e901a77a92f111440fdb1b593736b';
const UPDATER_PUBLIC_KEY = '${{ secrets.MDREADER_UPDATER_PUBLIC_KEY }}';

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

function steps(job: Mapping, path: string): Mapping[] {
  return list(job.steps, `${path}.steps`).map((entry) =>
    mapping(entry, `${path}.steps[]`),
  );
}

function exactMapping(
  value: unknown,
  expected: Mapping,
  path: string,
  message: string,
): void {
  const actual = mapping(value, path);
  const actualKeys = Object.keys(actual).sort();
  const expectedKeys = Object.keys(expected).sort();
  if (
    actualKeys.length !== expectedKeys.length ||
    actualKeys.some((key, index) => key !== expectedKeys[index]) ||
    expectedKeys.some(
      (key) => JSON.stringify(actual[key]) !== JSON.stringify(expected[key]),
    )
  ) {
    throw new Error(message);
  }
}

function requireNoEnvironment(step: Mapping, message: string): void {
  if (
    step.env !== undefined &&
    Object.keys(mapping(step.env, `step ${String(step.id)}.env`)).length > 0
  ) {
    throw new Error(message);
  }
}

function uniqueSteps(job: Mapping, path: string): Mapping[] {
  const entries = steps(job, path);
  const seen = new Set<string>();
  for (const step of entries) {
    if (typeof step.id !== 'string') continue;
    if (seen.has(step.id)) {
      throw new Error(`${path} step ${step.id} must exist exactly once`);
    }
    seen.add(step.id);
  }
  return entries;
}

function stepById(entries: Mapping[], id: string, message: string): Mapping {
  const matches = entries.filter((step) => step.id === id);
  if (matches.length !== 1) throw new Error(message);
  return matches[0]!;
}

function exactExecutable(
  entries: Mapping[],
  id: string,
  command: string,
  message: string,
  condition?: string,
): Mapping {
  const step = stepById(entries, id, message);
  if (
    typeof step.run !== 'string' ||
    step.run.trim() !== command ||
    (condition !== undefined && step.if !== condition)
  ) {
    throw new Error(message);
  }
  return step;
}

function exactAction(
  entries: Mapping[],
  id: string,
  action: string,
  inputs: Mapping,
  message: string,
): Mapping {
  const step = stepById(entries, id, message);
  if (step.uses !== action) throw new Error(message);
  exactMapping(step.with, inputs, `step ${id}.with`, message);
  return step;
}

function exactEnvironment(
  step: Mapping,
  expected: Mapping,
  message: string,
): void {
  exactMapping(step.env, expected, `step ${String(step.id)}.env`, message);
}

function needs(job: Mapping, path: string): string[] {
  if (typeof job.needs === 'string') return [job.needs];
  return list(job.needs, `${path}.needs`).map(String);
}

function exactNeeds(job: Mapping, expected: string[], path: string): void {
  const actual = needs(job, path).sort();
  const wanted = [...expected].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((entry, index) => entry !== wanted[index])
  ) {
    throw new Error(`${path} has an invalid dependency boundary`);
  }
}

function exactWindow(
  entries: Mapping[],
  first: string,
  last: string,
  expected: string[],
  message: string,
): void {
  const start = entries.findIndex((step) => step.id === first);
  const end = entries.findIndex((step) => step.id === last);
  const actual =
    start >= 0 && end >= start
      ? entries.slice(start, end + 1).map((step) => String(step.id))
      : [];
  if (
    actual.length !== expected.length ||
    actual.some((id, index) => id !== expected[index])
  ) {
    throw new Error(message);
  }
}

function assertPinnedActions(jobs: Mapping): void {
  for (const job of Object.values(jobs).map((entry) =>
    mapping(entry, 'jobs[]'),
  )) {
    for (const step of steps(job, 'jobs[]')) {
      if (
        typeof step.uses === 'string' &&
        !/@[0-9a-f]{40}$/.test(step.uses)
      ) {
        throw new Error(`release action is not pinned to a commit SHA: ${step.uses}`);
      }
      if (
        typeof step.run === 'string' &&
        /\b(?:echo|printf|printenv)\b[^\n]*\$\{\{\s*secrets(?:\.|\[)/.test(
          step.run,
        )
      ) {
        throw new Error('release commands must not print secret material');
      }
    }
  }
}

const AUTHORIZE_SOURCE = `if [[ "$GITHUB_EVENT_NAME" == "push" ]]; then
  [[ "$GITHUB_REF" =~ ^refs/tags/mdreader-v(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$ ]] || {
    echo "release tag is not an exact stable semantic version" >&2
    exit 1
  }
  git fetch --no-tags origin refs/heads/main:refs/remotes/origin/main
  git merge-base --is-ancestor "$GITHUB_SHA" refs/remotes/origin/main || {
    echo "release tag source is not contained in origin/main" >&2
    exit 1
  }
elif [[ "$GITHUB_EVENT_NAME" == "workflow_dispatch" ]]; then
  [[ "$GITHUB_REF" == "refs/heads/main" ]] || {
    echo "manual release verification must run from main" >&2
    exit 1
  }
else
  echo "unsupported release event $GITHUB_EVENT_NAME" >&2
  exit 1
fi`;

const METADATA = `if [[ "$GITHUB_EVENT_NAME" == "push" ]]; then
  VERSION="\${GITHUB_REF_NAME#mdreader-v}"
  PUBLISH_REQUESTED=true
else
  VERSION="$DISPATCH_VERSION"
  PUBLISH_REQUESTED=false
fi
WORKSPACE_VERSION="$(pnpm exec tsx -e "import { readWorkspaceVersion } from './scripts/release/release-config.ts'; process.stdout.write(readWorkspaceVersion('./Cargo.toml'))")"
[[ "$VERSION" == "$WORKSPACE_VERSION" ]] || {
  echo "release version does not match Cargo workspace version" >&2
  exit 1
}
{
  echo "version=$VERSION"
  echo "source_commit=$GITHUB_SHA"
  echo "published_at=$(date -u +'%Y-%m-%dT%H:%M:%S.000Z')"
  echo "publish_requested=$PUBLISH_REQUESTED"
} >> "$GITHUB_OUTPUT"`;

const BUILD_ENVIRONMENTS: Record<string, string> = {
  macos: 'mdreader-production-macos',
  windows: 'mdreader-production-windows',
  linux: 'mdreader-production-linux',
};

const COMMON_CREDENTIALS = {
  TAURI_SIGNING_PRIVATE_KEY:
    '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY }}',
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
    '${{ secrets.MDREADER_UPDATER_PRIVATE_KEY_PASSWORD }}',
  MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY,
};

const MAC_CREDENTIALS = {
  ...COMMON_CREDENTIALS,
  APPLE_CERTIFICATE: '${{ secrets.MDREADER_APPLE_CERTIFICATE }}',
  APPLE_CERTIFICATE_PASSWORD:
    '${{ secrets.MDREADER_APPLE_CERTIFICATE_PASSWORD }}',
  APPLE_SIGNING_IDENTITY:
    '${{ secrets.MDREADER_APPLE_SIGNING_IDENTITY }}',
  APPLE_ID: '${{ secrets.MDREADER_APPLE_ID }}',
  APPLE_PASSWORD: '${{ secrets.MDREADER_APPLE_PASSWORD }}',
  APPLE_TEAM_ID: '${{ secrets.MDREADER_APPLE_TEAM_ID }}',
};

const WINDOWS_CREDENTIALS = {
  ...COMMON_CREDENTIALS,
  WINDOWS_CERTIFICATE: '${{ secrets.MDREADER_WINDOWS_CERTIFICATE }}',
  WINDOWS_CERTIFICATE_PASSWORD:
    '${{ secrets.MDREADER_WINDOWS_CERTIFICATE_PASSWORD }}',
};

/**
 * Static fail-closed contract for the executable release workflow. Hosted
 * environment reviewers, deployment-branch rules, protected tags, real
 * signers, and R2 remain independent live controls.
 */
export function validateReleaseWorkflow(input: unknown): void {
  const workflow = mapping(input, 'workflow');
  exactMapping(
    workflow.permissions,
    { contents: 'read' },
    'workflow.permissions',
    'release workflow must use least privilege with contents: read',
  );
  exactMapping(
    workflow.concurrency,
    {
      group: 'mdreader-stable-release',
      'cancel-in-progress': false,
    },
    'workflow.concurrency',
    'release workflow must use one stable-channel concurrency group',
  );

  const triggers = mapping(workflow.on, 'workflow.on');
  exactMapping(
    mapping(triggers.push, 'workflow.on.push'),
    { tags: ['mdreader-v*'] },
    'workflow.on.push',
    'release workflow must use the reviewed protected tag namespace',
  );
  const dispatch = mapping(
    triggers.workflow_dispatch,
    'workflow.on.workflow_dispatch',
  );
  exactMapping(
    dispatch.inputs,
    {
      version: {
        description: 'Exact workspace semantic version',
        required: true,
        type: 'string',
      },
    },
    'workflow.on.workflow_dispatch.inputs',
    'manual release dispatch must remain verification-only',
  );

  const jobs = mapping(workflow.jobs, 'workflow.jobs');
  const prepare = mapping(jobs.prepare, 'jobs.prepare');
  const build = mapping(jobs.build, 'jobs.build');
  const aggregate = mapping(jobs.aggregate, 'jobs.aggregate');
  const publish = mapping(jobs.publish, 'jobs.publish');

  exactMapping(
    prepare.outputs,
    {
      version: '${{ steps.metadata.outputs.version }}',
      source_commit: '${{ steps.metadata.outputs.source_commit }}',
      published_at: '${{ steps.metadata.outputs.published_at }}',
      publish_requested: '${{ steps.metadata.outputs.publish_requested }}',
    },
    'jobs.prepare.outputs',
    'release prepare outputs are not exact',
  );
  const prepareSteps = uniqueSteps(prepare, 'jobs.prepare');
  exactAction(
    prepareSteps,
    'checkout-release-source',
    CHECKOUT,
    { 'persist-credentials': false, 'fetch-depth': 0 },
    'release source authorization requires the exact checkout boundary',
  );
  const authorize = exactExecutable(
    prepareSteps,
    'authorize-release-source',
    AUTHORIZE_SOURCE,
    'release workflow is missing the reviewed source authorization command',
  );
  void authorize;
  const metadata = exactExecutable(
    prepareSteps,
    'metadata',
    METADATA,
    'release workflow must keep manual dispatch dry-run and tags gated',
  );
  exactEnvironment(
    metadata,
    { DISPATCH_VERSION: '${{ inputs.version }}' },
    'release metadata environment is not least privilege',
  );

  if (build.environment !== '${{ matrix.environment }}') {
    throw new Error('release build must bind the exact protected signing environment');
  }
  const strategy = mapping(build.strategy, 'jobs.build.strategy');
  const matrix = mapping(strategy.matrix, 'jobs.build.strategy.matrix');
  const rows = list(matrix.include, 'jobs.build.strategy.matrix.include').map(
    (entry) => mapping(entry, 'matrix.include[]'),
  );
  const expectedRows: Record<string, Mapping> = {
    macos: {
      platform: 'macos',
      os: 'macos-14',
      environment: BUILD_ENVIRONMENTS.macos,
      target: 'universal-apple-darwin',
      bundles: 'dmg',
    },
    windows: {
      platform: 'windows',
      os: 'windows-2025',
      environment: BUILD_ENVIRONMENTS.windows,
      target: 'x86_64-pc-windows-msvc',
      bundles: 'nsis',
    },
    linux: {
      platform: 'linux',
      os: 'ubuntu-24.04',
      environment: BUILD_ENVIRONMENTS.linux,
      target: 'x86_64-unknown-linux-gnu',
      bundles: 'appimage,deb,rpm',
    },
  };
  if (rows.length !== 3) throw new Error('release matrix must contain exactly three platforms');
  for (const [platform, expected] of Object.entries(expectedRows)) {
    const row = rows.find((candidate) => candidate.platform === platform);
    if (!row) throw new Error(`${platform} release matrix entry is missing`);
    exactMapping(row, expected, `matrix.${platform}`, `${platform} release matrix is not exact`);
  }

  exactNeeds(build, ['prepare'], 'build');
  const buildSteps = uniqueSteps(build, 'jobs.build');
  exactAction(
    buildSteps,
    'checkout-build-source',
    CHECKOUT,
    { 'persist-credentials': false },
    'release build requires the exact source checkout boundary',
  );
  const macPresence = exactExecutable(
    buildSteps,
    'require-release-secrets-macos',
    'scripts/release/require-release-secrets.sh macos',
    'release workflow is missing fail-closed macOS signer credentials',
    "matrix.platform == 'macos'",
  );
  exactEnvironment(
    macPresence,
    MAC_CREDENTIALS,
    'macOS credential environment is not exact least privilege',
  );
  const windowsPresence = exactExecutable(
    buildSteps,
    'require-release-secrets-windows',
    'scripts/release/require-release-secrets.sh windows',
    'release workflow is missing fail-closed Windows signer credentials',
    "matrix.platform == 'windows'",
  );
  exactEnvironment(
    windowsPresence,
    WINDOWS_CREDENTIALS,
    'Windows credential environment is not exact least privilege',
  );
  const linuxPresence = exactExecutable(
    buildSteps,
    'require-release-secrets-linux',
    'scripts/release/require-release-secrets.sh linux',
    'release workflow is missing fail-closed Linux updater credentials',
    "matrix.platform == 'linux'",
  );
  exactEnvironment(
    linuxPresence,
    COMMON_CREDENTIALS,
    'Linux credential environment is not exact least privilege',
  );

  const macImport = stepById(
    buildSteps,
    'import-macos-cert',
    'release workflow is missing macOS certificate import',
  );
  if (macImport.if !== "matrix.platform == 'macos'") {
    throw new Error('macOS certificate import has an invalid condition');
  }
  exactEnvironment(
    macImport,
    {
      APPLE_CERTIFICATE: MAC_CREDENTIALS.APPLE_CERTIFICATE,
      APPLE_CERTIFICATE_PASSWORD: MAC_CREDENTIALS.APPLE_CERTIFICATE_PASSWORD,
    },
    'macOS certificate import environment is not exact least privilege',
  );
  const windowsInspect = stepById(
    buildSteps,
    'inspect-windows-cert',
    'release workflow is missing ephemeral Windows certificate inspection',
  );
  if (
    windowsInspect.if !== "matrix.platform == 'windows'" ||
    windowsInspect.shell !== 'pwsh'
  ) {
    throw new Error('Windows certificate inspection has an invalid execution boundary');
  }
  exactEnvironment(
    windowsInspect,
    {
      WINDOWS_CERTIFICATE: WINDOWS_CREDENTIALS.WINDOWS_CERTIFICATE,
      WINDOWS_CERTIFICATE_PASSWORD:
        WINDOWS_CREDENTIALS.WINDOWS_CERTIFICATE_PASSWORD,
    },
    'Windows certificate inspection environment is not exact least privilege',
  );
  const windowsImport = exactExecutable(
    buildSteps,
    'import-windows-cert',
    `$path = Join-Path $env:RUNNER_TEMP 'mdreader-release.pfx'
$beforePath = Join-Path $env:RUNNER_TEMP 'mdreader-release-cert-before.txt'
@(Get-ChildItem Cert:\\CurrentUser\\My | ForEach-Object Thumbprint) | Set-Content $beforePath
[IO.File]::WriteAllBytes($path, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE))
$password = ConvertTo-SecureString $env:WINDOWS_CERTIFICATE_PASSWORD -AsPlainText -Force
$imported = @(Import-PfxCertificate -FilePath $path -Password $password -CertStoreLocation Cert:\\CurrentUser\\My)
$certificate = @($imported | Where-Object Thumbprint -eq "\${{ steps.inspect-windows-cert.outputs.thumbprint }}")
if ($certificate.Count -ne 1) { throw "persisted Windows signing certificate does not match inspected thumbprint" }
if (-not $certificate.HasPrivateKey) { throw "Windows signing certificate has no private key" }`,
    'release workflow is missing Windows certificate import',
    "matrix.platform == 'windows'",
  );
  if (
    windowsImport.shell !== 'pwsh'
  ) {
    throw new Error('Windows certificate import has an invalid execution boundary');
  }
  exactEnvironment(
    windowsImport,
    {
      WINDOWS_CERTIFICATE: WINDOWS_CREDENTIALS.WINDOWS_CERTIFICATE,
      WINDOWS_CERTIFICATE_PASSWORD:
        WINDOWS_CREDENTIALS.WINDOWS_CERTIFICATE_PASSWORD,
    },
    'Windows certificate import environment is not exact least privilege',
  );

  const macConfig = exactExecutable(
    buildSteps,
    'generate-release-config-macos',
    'scripts/release/generate-direct-release-config.sh macos "${{ needs.prepare.outputs.version }}" "$RUNNER_TEMP/mdreader-release-config.json"',
    'release workflow is missing the reviewed macOS release config generation',
    "matrix.platform == 'macos'",
  );
  exactEnvironment(
    macConfig,
    {
      MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY,
      APPLE_SIGNING_IDENTITY:
        '${{ secrets.MDREADER_APPLE_SIGNING_IDENTITY }}',
    },
    'macOS release config environment is not exact least privilege',
  );
  const windowsConfig = exactExecutable(
    buildSteps,
    'generate-release-config-windows',
    'scripts/release/generate-direct-release-config.sh windows "${{ needs.prepare.outputs.version }}" "$RUNNER_TEMP/mdreader-release-config.json"',
    'release workflow is missing the reviewed Windows release config generation',
    "matrix.platform == 'windows'",
  );
  exactEnvironment(
    windowsConfig,
    {
      MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY,
      WINDOWS_CERT_THUMBPRINT:
        '${{ steps.inspect-windows-cert.outputs.thumbprint }}',
    },
    'Windows release config environment is not exact least privilege',
  );
  const linuxConfig = exactExecutable(
    buildSteps,
    'generate-release-config-linux',
    'scripts/release/generate-direct-release-config.sh linux "${{ needs.prepare.outputs.version }}" "$RUNNER_TEMP/mdreader-release-config.json"',
    'release workflow is missing the reviewed Linux release config generation',
    "matrix.platform == 'linux'",
  );
  exactEnvironment(
    linuxConfig,
    {
      MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY,
    },
    'Linux release config environment is not exact least privilege',
  );

  const prepareBuild = exactExecutable(
    buildSteps,
    'prepare-direct-release',
    'scripts/release/prepare-direct-release.sh "${{ matrix.target }}" "${{ matrix.bundles }}" "$RUNNER_TEMP/mdreader-release-config.json" "$RUNNER_TEMP/mdreader-release-attestation.json"',
    'release workflow is missing the unprivileged frontend and Rust build',
  );
  requireNoEnvironment(
    prepareBuild,
    'unprivileged release build must not receive signing credentials',
  );

  const macBundle = exactExecutable(
    buildSteps,
    'bundle-direct-release-macos',
    'scripts/release/bundle-direct-release.sh "${{ matrix.target }}" "${{ matrix.bundles }}" "$RUNNER_TEMP/mdreader-release-config.json"',
    'release workflow is missing the minimal macOS signer/bundler',
    "matrix.platform == 'macos'",
  );
  exactEnvironment(
    macBundle,
    {
      TAURI_SIGNING_PRIVATE_KEY:
        COMMON_CREDENTIALS.TAURI_SIGNING_PRIVATE_KEY,
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
        COMMON_CREDENTIALS.TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
      APPLE_ID: MAC_CREDENTIALS.APPLE_ID,
      APPLE_PASSWORD: MAC_CREDENTIALS.APPLE_PASSWORD,
      APPLE_TEAM_ID: MAC_CREDENTIALS.APPLE_TEAM_ID,
    },
    'macOS signer environment is not exact least privilege',
  );

  const macCleanup = exactExecutable(
    buildSteps,
    'cleanup-macos-cert',
    `security delete-keychain "$RUNNER_TEMP/mdreader-release.keychain-db" 2>/dev/null || true
rm -f "$RUNNER_TEMP/mdreader-release.p12"`,
    'release workflow is missing fail-closed macOS certificate cleanup',
    "always() && matrix.platform == 'macos'",
  );
  requireNoEnvironment(
    macCleanup,
    'macOS certificate cleanup must not receive credentials',
  );
  const windowsCleanup = exactExecutable(
    buildSteps,
    'cleanup-windows-cert',
    `$beforePath = Join-Path $env:RUNNER_TEMP 'mdreader-release-cert-before.txt'
if (Test-Path $beforePath) {
  $before = @(Get-Content $beforePath)
  Get-ChildItem Cert:\\CurrentUser\\My |
    Where-Object { $before -notcontains $_.Thumbprint } |
    ForEach-Object { Remove-Item $_.PSPath -Force -ErrorAction SilentlyContinue }
}
Remove-Item (Join-Path $env:RUNNER_TEMP 'mdreader-release.pfx') -Force -ErrorAction SilentlyContinue
Remove-Item $beforePath -Force -ErrorAction SilentlyContinue`,
    'release workflow is missing fail-closed Windows certificate cleanup',
    "always() && matrix.platform == 'windows'",
  );
  if (windowsCleanup.shell !== 'pwsh') {
    throw new Error('Windows certificate cleanup must use the reviewed PowerShell boundary');
  }
  requireNoEnvironment(
    windowsCleanup,
    'Windows certificate cleanup must not receive credentials',
  );
  const windowsBundle = exactExecutable(
    buildSteps,
    'bundle-direct-release-windows',
    'scripts/release/bundle-direct-release.sh "${{ matrix.target }}" "${{ matrix.bundles }}" "$RUNNER_TEMP/mdreader-release-config.json"',
    'release workflow is missing the minimal Windows signer/bundler',
    "matrix.platform == 'windows'",
  );
  exactEnvironment(
    windowsBundle,
    {
      TAURI_SIGNING_PRIVATE_KEY:
        COMMON_CREDENTIALS.TAURI_SIGNING_PRIVATE_KEY,
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
        COMMON_CREDENTIALS.TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    },
    'Windows signer environment is not exact least privilege',
  );
  const linuxBundle = exactExecutable(
    buildSteps,
    'bundle-direct-release-linux',
    'scripts/release/bundle-direct-release.sh "${{ matrix.target }}" "${{ matrix.bundles }}" "$RUNNER_TEMP/mdreader-release-config.json"',
    'release workflow is missing the minimal Linux signer/bundler',
    "matrix.platform == 'linux'",
  );
  exactEnvironment(
    linuxBundle,
    {
      TAURI_SIGNING_PRIVATE_KEY:
        COMMON_CREDENTIALS.TAURI_SIGNING_PRIVATE_KEY,
      TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
        COMMON_CREDENTIALS.TAURI_SIGNING_PRIVATE_KEY_PASSWORD,
    },
    'Linux signer environment is not exact least privilege',
  );

  const verifyBuild = exactExecutable(
    buildSteps,
    'verify-direct-release',
    'scripts/release/verify-direct-release.sh "${{ matrix.target }}" "${{ matrix.bundles }}" "$RUNNER_TEMP/mdreader-release-config.json" "$RUNNER_TEMP/mdreader-release-attestation.json" "$RUNNER_TEMP/mdreader-release-bundle-seal.json"',
    'release workflow is missing unprivileged post-bundle attestation',
  );
  requireNoEnvironment(
    verifyBuild,
    'release verifier must not receive signing credentials',
  );

  for (const [id, command, condition, message] of [
    [
      'macos-verify-signature',
      'scripts/release/verify-macos-signature.sh "target/${{ matrix.target }}/release/bundle"',
      "matrix.platform == 'macos'",
      'release workflow is missing executable macOS signature verification',
    ],
    [
      'macos-verify-notarization',
      'scripts/release/verify-macos-notarization.sh "target/${{ matrix.target }}/release/bundle"',
      "matrix.platform == 'macos'",
      'release workflow is missing executable macOS notarization verification',
    ],
    [
      'windows-verify-signature',
      'scripts/release/verify-windows-signature.ps1 -BundleRoot "target/${{ matrix.target }}/release/bundle"',
      "matrix.platform == 'windows'",
      'release workflow is missing executable Windows signing verification',
    ],
    [
      'linux-verify-packages',
      'scripts/release/verify-linux-packages.sh "target/${{ matrix.target }}/release/bundle" "$RUNNER_TEMP/mdreader-readelf.txt"',
      "matrix.platform == 'linux'",
      'release workflow is missing executable Linux package verification',
    ],
  ] as const) {
    const verification = exactExecutable(
      buildSteps,
      id,
      command,
      message,
      condition,
    );
    requireNoEnvironment(
      verification,
      `${id} must not receive signing credentials`,
    );
  }

  exactAction(
    buildSteps,
    'install-syft',
    SYFT,
    { 'syft-version': 'v1.29.0' },
    'release workflow must install the pinned Syft scanner',
  );
  const stage = exactExecutable(
    buildSteps,
    'stage-release',
    'scripts/release/stage-platform.sh "${{ matrix.platform }}" "${{ needs.prepare.outputs.version }}" "${{ needs.prepare.outputs.source_commit }}" "${{ needs.prepare.outputs.published_at }}" "${{ matrix.target }}" "${{ matrix.bundles }}" "$RUNNER_TEMP/mdreader-release-config.json" "$RUNNER_TEMP/mdreader-release-attestation.json" "$RUNNER_TEMP/mdreader-release-bundle-seal.json"',
    'release workflow is missing executable checksum staging',
  );
  exactEnvironment(
    stage,
    {
      SYFT_BIN: '${{ steps.install-syft.outputs.cmd }}',
      MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY,
    },
    'stage release must bind the exact Syft output and updater public key',
  );
  const sbom = exactExecutable(
    buildSteps,
    'generate-sbom',
    'scripts/release/inspect-sboms.sh target/release-stage',
    'release workflow is missing executable SBOM generation',
  );
  requireNoEnvironment(sbom, 'SBOM inspection must not receive credentials');
  const staged = exactExecutable(
    buildSteps,
    'validate-staged-release',
    'scripts/release/validate-staged-platform.sh "${{ matrix.platform }}" target/release-stage',
    'release workflow is missing executable updater signature validation',
  );
  requireNoEnvironment(
    staged,
    'staged release validation must not receive credentials',
  );
  exactAction(
    buildSteps,
    'upload-staged-release',
    UPLOAD,
    {
      name: 'mdreader-release-${{ matrix.platform }}',
      path: 'target/release-stage',
      'if-no-files-found': 'error',
      'retention-days': 7,
    },
    'release workflow is missing the exact verified platform upload action',
  );
  exactWindow(
    buildSteps,
    'generate-release-config-macos',
    'upload-staged-release',
    [
      'generate-release-config-macos',
      'generate-release-config-windows',
      'generate-release-config-linux',
      'prepare-direct-release',
      'import-macos-cert',
      'import-windows-cert',
      'bundle-direct-release-macos',
      'bundle-direct-release-windows',
      'bundle-direct-release-linux',
      'cleanup-macos-cert',
      'cleanup-windows-cert',
      'verify-direct-release',
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
    'release config build window contains an unreviewed mutation step',
  );

  exactNeeds(aggregate, ['prepare', 'build'], 'aggregate');
  if (aggregate.environment !== undefined) {
    throw new Error('aggregate verification must not enter a protected credential environment');
  }
  exactMapping(
    aggregate.outputs,
    {
      publish_plan_sha256:
        '${{ steps.seal-publication.outputs.publish_plan_sha256 }}',
      publisher_sha256:
        '${{ steps.bundle-publisher.outputs.publisher_sha256 }}',
    },
    'jobs.aggregate.outputs',
    'release aggregate outputs are not exact',
  );
  const aggregateSteps = uniqueSteps(aggregate, 'jobs.aggregate');
  exactAction(
    aggregateSteps,
    'checkout-aggregate-source',
    CHECKOUT,
    { 'persist-credentials': false },
    'release aggregate requires the exact source checkout boundary',
  );
  exactAction(
    aggregateSteps,
    'download-staged-releases',
    DOWNLOAD,
    {
      pattern: 'mdreader-release-*',
      path: 'target/release-aggregate',
      'merge-multiple': true,
    },
    'release workflow is missing the exact verified platform download action',
  );
  const assemble = exactExecutable(
    aggregateSteps,
    'assemble-manifest',
    `pnpm exec tsx scripts/release/release-cli.ts assemble \\
  --root target/release-aggregate \\
  --version "\${{ needs.prepare.outputs.version }}" \\
  --source-commit "\${{ needs.prepare.outputs.source_commit }}" \\
  --published-at "\${{ needs.prepare.outputs.published_at }}"`,
    'release workflow is missing the reviewed aggregate assembly command',
  );
  exactEnvironment(
    assemble,
    { MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY },
    'aggregate assembly environment is not exact',
  );
  const validate = exactExecutable(
    aggregateSteps,
    'validate-release',
    `pnpm exec tsx scripts/release/release-cli.ts validate \\
  --root target/release-aggregate \\
  --manifest "target/release-aggregate/releases/v\${{ needs.prepare.outputs.version }}/manifest.json" \\
  --source-commit "\${{ needs.prepare.outputs.source_commit }}"`,
    'release workflow is missing the reviewed aggregate validation command',
  );
  exactEnvironment(
    validate,
    { MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY },
    'aggregate validation environment is not exact',
  );
  const seal = exactExecutable(
    aggregateSteps,
    'seal-publication',
    `PLAN_SHA256="$(pnpm exec tsx scripts/release/release-cli.ts seal-publication \\
  --root target/release-aggregate \\
  --manifest "target/release-aggregate/releases/v\${{ needs.prepare.outputs.version }}/manifest.json" \\
  --source-commit "\${{ needs.prepare.outputs.source_commit }}" \\
  --version "\${{ needs.prepare.outputs.version }}" \\
  --output target/release-publish/sealed-publication-plan.json)"
echo "publish_plan_sha256=$PLAN_SHA256" >> "$GITHUB_OUTPUT"`,
    'release workflow is missing the reviewed sealed publication plan',
  );
  exactEnvironment(
    seal,
    { MDREADER_UPDATER_PUBLIC_KEY: UPDATER_PUBLIC_KEY },
    'sealed plan environment is not exact',
  );
  const publisherBundle = exactExecutable(
    aggregateSteps,
    'bundle-publisher',
    'scripts/release/build-sealed-publisher.sh target/release-publish/publish-sealed-plan.mjs',
    'release workflow is missing the reviewed no-child-process publisher build',
  );
  requireNoEnvironment(
    publisherBundle,
    'publisher build must not receive publication credentials',
  );
  exactAction(
    aggregateSteps,
    'upload-publish-plan',
    UPLOAD,
    {
      name:
        'mdreader-sealed-publication-${{ needs.prepare.outputs.source_commit }}',
      path: 'target/release-publish',
      'if-no-files-found': 'error',
      'retention-days': 1,
    },
    'release workflow is missing the exact sealed publication upload action',
  );
  exactWindow(
    aggregateSteps,
    'download-staged-releases',
    'upload-publish-plan',
    [
      'download-staged-releases',
      'assemble-manifest',
      'validate-release',
      'seal-publication',
      'bundle-publisher',
      'upload-publish-plan',
    ],
    'aggregate verification and sealing window contains an unreviewed step',
  );

  exactNeeds(publish, ['prepare', 'aggregate'], 'publish');
  if (
    publish.environment !== 'mdreader-production-publication' ||
    publish.if !== "needs.prepare.outputs.publish_requested == 'true'"
  ) {
    throw new Error(
      'live publication requires the exact protected environment and approved tag request',
    );
  }
  const publishMetadata = Object.fromEntries(
    Object.entries(publish).filter(([name]) => name !== 'steps'),
  );
  if (/\$\{\{\s*secrets(?:\.|\[)/.test(JSON.stringify(publishMetadata))) {
    throw new Error('release publish job must not expose ambient secret material');
  }
  const publishSteps = uniqueSteps(publish, 'jobs.publish');
  for (const step of publishSteps) {
    if (
      step.id !== 'publish-r2' &&
      /\$\{\{\s*secrets(?:\.|\[)/.test(JSON.stringify(step))
    ) {
      throw new Error(
        'unreviewed publish step must not receive secret material or publication credentials',
      );
    }
  }
  if (publishSteps.length !== 2) {
    throw new Error('protected publication job contains an unreviewed step');
  }
  exactAction(
    publishSteps,
    'download-publish-plan',
    DOWNLOAD,
    {
      name:
        'mdreader-sealed-publication-${{ needs.prepare.outputs.source_commit }}',
      path: 'target/release-publish',
    },
    'release workflow is missing the exact sealed publication download action',
  );
  const publisher = exactExecutable(
    publishSteps,
    'publish-r2',
    'exec node target/release-publish/publish-sealed-plan.mjs --plan target/release-publish/sealed-publication-plan.json --plan-sha256 "${{ needs.aggregate.outputs.publish_plan_sha256 }}" --publisher-sha256 "${{ needs.aggregate.outputs.publisher_sha256 }}" --source-commit "${{ needs.prepare.outputs.source_commit }}" --version "${{ needs.prepare.outputs.version }}" --bucket "$RELEASE_BUCKET"',
    'release workflow is missing the reviewed sealed R2 publication command',
  );
  exactEnvironment(
    publisher,
    {
      CLOUDFLARE_ACCOUNT_ID: '${{ secrets.CLOUDFLARE_ACCOUNT_ID }}',
      MDREADER_R2_ACCESS_KEY_ID:
        '${{ secrets.MDREADER_R2_ACCESS_KEY_ID }}',
      MDREADER_R2_SECRET_ACCESS_KEY:
        '${{ secrets.MDREADER_R2_SECRET_ACCESS_KEY }}',
      RELEASE_BUCKET: '${{ vars.MDREADER_RELEASE_BUCKET }}',
    },
    'R2 publisher environment is not exact least privilege',
  );

  assertPinnedActions(jobs);
}
