type Mapping = Record<string, unknown>;

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

function stepIds(job: Mapping): Set<string> {
  return new Set(
    list(job.steps, 'job.steps')
      .map((entry) => mapping(entry, 'job.steps[]').id)
      .filter((id): id is string => typeof id === 'string'),
  );
}

function requireStep(ids: Set<string>, id: string, message: string): void {
  if (!ids.has(id)) throw new Error(message);
}

function requireExecutableStep(
  job: Mapping,
  id: string,
  required: RegExp[],
  message: string,
  expectedCondition?: string,
): void {
  const step = list(job.steps, 'job.steps')
    .map((entry) => mapping(entry, 'job.steps[]'))
    .find((entry) => entry.id === id);
  const command = typeof step?.run === 'string' ? step.run : '';
  if (
    !step ||
    required.some((pattern) => !pattern.test(command)) ||
    (expectedCondition !== undefined && step.if !== expectedCondition)
  ) {
    throw new Error(message);
  }
}

function requireActionStep(
  job: Mapping,
  id: string,
  action: string,
  inputName: string,
  inputValue: string,
  message: string,
): void {
  const step = list(job.steps, 'job.steps')
    .map((entry) => mapping(entry, 'job.steps[]'))
    .find((entry) => entry.id === id);
  const withValues = step ? mapping(step.with, `step ${id}.with`) : {};
  if (
    !step ||
    step.uses !== action ||
    withValues[inputName] !== inputValue
  ) {
    throw new Error(message);
  }
}

function needs(job: Mapping): string[] {
  if (typeof job.needs === 'string') return [job.needs];
  return list(job.needs, 'publish.needs').map((entry) => String(entry));
}

function includesBundles(entry: Mapping, required: string[]): boolean {
  const bundles = entry.bundles;
  return (
    typeof bundles === 'string' &&
    required.every((bundle) => bundles.split(',').map((item) => item.trim()).includes(bundle))
  );
}

/**
 * Static fail-closed contract for the executable release workflow. This is not
 * a substitute for live platform gates; it prevents deleting their stages or
 * moving publication ahead of the aggregate verification job.
 */
export function validateReleaseWorkflow(input: unknown): void {
  const workflow = mapping(input, 'workflow');
  const permissions = mapping(workflow.permissions, 'workflow.permissions');
  if (
    permissions.contents !== 'read' ||
    Object.entries(permissions).some(
      ([name, value]) => name !== 'contents' && value !== 'none',
    )
  ) {
    throw new Error('release workflow must use least privilege with contents: read');
  }
  const concurrency = mapping(workflow.concurrency, 'workflow.concurrency');
  if (
    concurrency.group !== 'mdreader-stable-release' ||
    concurrency['cancel-in-progress'] !== false
  ) {
    throw new Error('release workflow must use one stable-channel concurrency group');
  }

  const jobs = mapping(workflow.jobs, 'workflow.jobs');
  const prepare = mapping(jobs.prepare, 'jobs.prepare');
  const build = mapping(jobs.build, 'jobs.build');
  const publish = mapping(jobs.publish, 'jobs.publish');
  void prepare;

  const strategy = mapping(build.strategy, 'jobs.build.strategy');
  const matrix = mapping(strategy.matrix, 'jobs.build.strategy.matrix');
  const rows = list(matrix.include, 'jobs.build.strategy.matrix.include').map((entry) =>
    mapping(entry, 'matrix.include[]'),
  );
  const macos = rows.find((entry) => entry.platform === 'macos');
  if (
    !macos ||
    macos.target !== 'universal-apple-darwin' ||
    !includesBundles(macos, ['dmg'])
  ) {
    throw new Error('macOS release matrix must build a universal DMG');
  }
  const windows = rows.find((entry) => entry.platform === 'windows');
  if (
    !windows ||
    windows.target !== 'x86_64-pc-windows-msvc' ||
    !includesBundles(windows, ['nsis'])
  ) {
    throw new Error('Windows release matrix must build signed x64 NSIS');
  }
  const linux = rows.find((entry) => entry.platform === 'linux');
  if (
    !linux ||
    linux.target !== 'x86_64-unknown-linux-gnu' ||
    !includesBundles(linux, ['appimage', 'deb', 'rpm'])
  ) {
    throw new Error('Linux release matrix must build AppImage, DEB, and RPM');
  }

  const buildIds = stepIds(build);
  requireStep(buildIds, 'require-release-secrets', 'release workflow is missing fail-closed signer credentials');
  requireStep(buildIds, 'build-direct-release', 'release workflow is missing the signed updater build');
  requireStep(buildIds, 'macos-verify-signature', 'release workflow is missing macOS signature verification');
  requireStep(buildIds, 'macos-verify-notarization', 'release workflow is missing macOS notarization verification');
  requireStep(buildIds, 'windows-verify-signature', 'release workflow is missing Windows signing verification');
  requireStep(buildIds, 'linux-verify-packages', 'release workflow is missing Linux package verification');
  requireStep(buildIds, 'install-syft', 'release workflow is missing pinned Syft installation');
  requireStep(buildIds, 'generate-sbom', 'release workflow is missing SBOM generation');
  requireStep(buildIds, 'stage-release', 'release workflow is missing checksum and staging verification');
  requireStep(buildIds, 'validate-staged-release', 'release workflow is missing updater signature validation');
  requireStep(buildIds, 'upload-staged-release', 'release workflow is missing verified artifact handoff');
  requireActionStep(
    build,
    'install-syft',
    'anchore/sbom-action/download-syft@f8bdd1d8ac5e901a77a92f111440fdb1b593736b',
    'syft-version',
    'v1.29.0',
    'release workflow must install the pinned Syft scanner',
  );
  requireExecutableStep(
    build,
    'macos-verify-signature',
    [/codesign\s+--verify/, /spctl\s+--assess/],
    'release workflow is missing executable macOS signature verification',
    "matrix.platform == 'macos'",
  );
  requireExecutableStep(
    build,
    'macos-verify-notarization',
    [/stapler\s+validate/, /spctl\s+--assess/],
    'release workflow is missing executable macOS notarization verification',
    "matrix.platform == 'macos'",
  );
  requireExecutableStep(
    build,
    'windows-verify-signature',
    [/Get-AuthenticodeSignature/, /Status\s+-ne\s+'Valid'/],
    'release workflow is missing executable Windows signing verification',
    "matrix.platform == 'windows'",
  );
  requireExecutableStep(
    build,
    'linux-verify-packages',
    [
      /--appimage-extract/,
      /dpkg-deb\s+--info/,
      /rpm\s+-qpi/,
      /release-cli\.ts\s+verify-linux-abi/,
    ],
    'release workflow is missing executable Linux package verification',
    "matrix.platform == 'linux'",
  );
  requireExecutableStep(
    build,
    'generate-sbom',
    [/spdxVersion/, /SBOM/],
    'release workflow is missing executable SBOM generation',
  );
  requireExecutableStep(
    build,
    'stage-release',
    [/release-cli\.ts\s+stage/, /--source-commit/],
    'release workflow is missing executable checksum staging',
  );
  requireExecutableStep(
    build,
    'validate-staged-release',
    [/signature/, /sha256|checksum/i],
    'release workflow is missing executable updater signature validation',
  );

  const dependencies = needs(publish);
  if (!dependencies.includes('prepare') || !dependencies.includes('build')) {
    throw new Error('publication must depend on all platform builds and preparation');
  }
  const publishSteps = list(publish.steps, 'jobs.publish.steps').map((entry) =>
    mapping(entry, 'publish.steps[]'),
  );
  const index = (id: string) => publishSteps.findIndex((step) => step.id === id);
  if (
    index('assemble-manifest') < 0 ||
    index('validate-release') < 0 ||
    index('publish-r2') < 0 ||
    !(index('assemble-manifest') < index('validate-release') &&
      index('validate-release') < index('publish-r2'))
  ) {
    throw new Error('release workflow must enforce aggregate verification before publication');
  }

  for (const job of Object.values(jobs).map((entry) => mapping(entry, 'jobs[]'))) {
    for (const step of list(job.steps, 'jobs[].steps').map((entry) =>
      mapping(entry, 'jobs[].steps[]'),
    )) {
      if (
        typeof step.run === 'string' &&
        /\b(?:echo|printf|printenv)\b[^\n]*\$\{\{\s*secrets\./.test(step.run)
      ) {
        throw new Error('release commands must not print secret material');
      }
      if (
        typeof step.uses === 'string' &&
        !/@[0-9a-f]{40}$/.test(step.uses)
      ) {
        throw new Error(`release action is not pinned to a commit SHA: ${step.uses}`);
      }
    }
  }
}
