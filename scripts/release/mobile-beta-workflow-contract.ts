type Mapping = Record<string, unknown>;

const PRODUCTION_PACKAGE_ID = 'dev.mdreader.app';
const PUBLISH_ENVIRONMENT = 'mdreader-mobile-beta';
const PINNED_ACTIONS = new Set([
  'actions/checkout@11d5960a326750d5838078e36cf38b85af677262',
  'actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020',
  'actions/setup-java@c1e323688fd81a25caa38c78aa6df2d33d3e20d9',
  'actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02',
  'actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093',
  'dtolnay/rust-toolchain@4cda84d5c5c54efe2404f9d843567869ab1699d4',
]);

const REQUIRED_STEPS: Record<string, readonly string[]> = {
  prepare: [
    'checkout-source',
    'setup-node',
    'authorize-source-and-release',
    'install-dependencies',
    'validate-workflow',
  ],
  'build-ios-unsigned': [
    'checkout-source',
    'setup-node',
    'setup-rust',
    'install-dependencies',
    'build-ios-archive',
    'seal-ios-handoff',
    'upload-ios-handoff',
  ],
  'build-android-unsigned': [
    'checkout-source',
    'setup-node',
    'setup-java',
    'setup-rust',
    'install-dependencies',
    'build-android-bundle',
    'seal-android-handoff',
    'upload-android-handoff',
  ],
  'publish-ios-beta': [
    'checkout-source',
    'setup-node',
    'install-dependencies',
    'download-ios-handoff',
    'verify-ios-handoff',
    'sign-ios',
    'inspect-ios',
    'upload-testflight',
    'upload-ios-evidence',
  ],
  'publish-android-beta': [
    'checkout-source',
    'setup-node',
    'install-dependencies',
    'setup-java',
    'download-android-handoff',
    'verify-android-handoff',
    'install-bundletool',
    'sign-android',
    'inspect-android',
    'upload-google-play',
    'upload-android-evidence',
  ],
};

function mapping(value: unknown, label: string): Mapping {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error(`${label} must be a mapping`);
  }
  return value as Mapping;
}

function array(value: unknown, label: string): Mapping[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value.map((entry, index) => mapping(entry, `${label}[${index}]`));
}

function scalar(value: unknown, label: string): string {
  if (typeof value !== 'string') throw new Error(`${label} must be a string`);
  return value;
}

function exactKeys(
  value: Mapping,
  expected: readonly string[],
  label: string,
): void {
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    throw new Error(`${label} keys differ from the reviewed contract`);
  }
}

function serialized(value: unknown): string {
  return JSON.stringify(value);
}

function stepById(job: Mapping, jobName: string): Map<string, Mapping> {
  const steps = array(job.steps, `${jobName}.steps`);
  const indexed = new Map<string, Mapping>();
  for (const step of steps) {
    const id = scalar(step.id, `${jobName} step id`);
    if (indexed.has(id)) throw new Error(`${jobName} duplicates step ${id}`);
    indexed.set(id, step);
    if (typeof step.uses === 'string' && !PINNED_ACTIONS.has(step.uses)) {
      throw new Error(`${jobName}/${id} does not use a reviewed pinned action`);
    }
  }
  const required = REQUIRED_STEPS[jobName];
  if (!required) throw new Error(`unexpected mobile beta job ${jobName}`);
  exactKeys(
    Object.fromEntries(indexed),
    required,
    `${jobName} step identifiers`,
  );
  return indexed;
}

function requireRun(
  steps: Map<string, Mapping>,
  id: string,
  fragments: readonly string[],
): void {
  const run = scalar(steps.get(id)?.run, `${id}.run`);
  for (const fragment of fragments) {
    if (!run.includes(fragment)) {
      throw new Error(`${id} is missing required release boundary ${fragment}`);
    }
  }
}

/**
 * Validates the exact production mobile-beta workflow shape. This is
 * intentionally narrower than GitHub's workflow schema: every accepted job,
 * action, authority boundary, and handoff is enumerated here.
 */
export function validateMobileBetaWorkflow(document: unknown): void {
  const root = mapping(document, 'workflow');
  exactKeys(
    root,
    ['name', 'on', 'permissions', 'concurrency', 'jobs'],
    'workflow',
  );
  if (root.name !== 'MDReader production mobile beta') {
    throw new Error('workflow name differs from the reviewed contract');
  }
  const permissions = mapping(root.permissions, 'permissions');
  exactKeys(permissions, ['contents'], 'permissions');
  if (permissions.contents !== 'read') {
    throw new Error('mobile beta workflow may only read repository contents');
  }
  const triggers = mapping(root.on, 'on');
  exactKeys(triggers, ['workflow_dispatch'], 'workflow triggers');
  const dispatch = mapping(triggers.workflow_dispatch, 'workflow_dispatch');
  const inputs = mapping(dispatch.inputs, 'workflow_dispatch.inputs');
  exactKeys(inputs, ['version', 'build_number', 'publish'], 'workflow inputs');
  if (
    mapping(inputs.publish, 'publish input').type !== 'boolean' ||
    mapping(inputs.publish, 'publish input').default !== false
  ) {
    throw new Error('publishing must be an explicit false-by-default boolean');
  }

  const jobs = mapping(root.jobs, 'jobs');
  exactKeys(jobs, Object.keys(REQUIRED_STEPS), 'mobile beta jobs');
  const indexed = new Map<string, Map<string, Mapping>>();
  for (const [name, value] of Object.entries(jobs)) {
    indexed.set(name, stepById(mapping(value, name), name));
  }

  for (const name of ['prepare', 'build-ios-unsigned', 'build-android-unsigned']) {
    if (serialized(jobs[name]).includes('secrets.')) {
      throw new Error(`${name} may not receive signing or provider credentials`);
    }
    if ('environment' in mapping(jobs[name], name)) {
      throw new Error(`${name} may not enter a credentialed environment`);
    }
  }
  for (const name of ['publish-ios-beta', 'publish-android-beta']) {
    const job = mapping(jobs[name], name);
    if (job.environment !== PUBLISH_ENVIRONMENT) {
      throw new Error(`${name} must use the protected publishing environment`);
    }
    if (!scalar(job.if, `${name}.if`).includes('inputs.publish == true')) {
      throw new Error(`${name} must be disabled unless publishing is explicit`);
    }
    requireRun(indexed.get(name)!, 'install-dependencies', [
      'corepack enable',
      'pnpm install --frozen-lockfile',
    ]);
  }
  if (
    serialized(jobs.prepare).includes('MDREADER_GOOGLE_PLAY_CLOSED_TRACK') ||
    serialized(jobs.prepare).includes('google_closed_track')
  ) {
    throw new Error('Google Play track may only resolve inside the protected publisher');
  }

  const iosBuild = indexed.get('build-ios-unsigned')!;
  requireRun(iosBuild, 'build-ios-archive', [
    PRODUCTION_PACKAGE_ID,
    'bundleVersion',
    'ios-release-config.json',
    '--target aarch64',
    '--no-sign',
    '--archive-only',
  ]);
  const androidBuild = indexed.get('build-android-unsigned')!;
  requireRun(androidBuild, 'build-android-bundle', [
    PRODUCTION_PACKAGE_ID,
    '--target aarch64',
    '--aab',
  ]);
  requireRun(indexed.get('publish-ios-beta')!, 'inspect-ios', [
    'inspect-ios',
    '--snapshot',
    '--manifest',
    '--provisioning-profile-sha256',
  ]);
  const profileEnvironment = mapping(
    indexed.get('publish-ios-beta')!.get('inspect-ios')!.env,
    'inspect-ios.env',
  );
  exactKeys(
    profileEnvironment,
    ['IOS_PROVISIONING_PROFILE_SHA256'],
    'inspect-ios.env',
  );
  if (
    profileEnvironment.IOS_PROVISIONING_PROFILE_SHA256 !==
    '${{ vars.MDREADER_IOS_PROVISIONING_PROFILE_SHA256 }}'
  ) {
    throw new Error(
      'iOS publisher must use the protected reviewed provisioning-profile digest',
    );
  }
  requireRun(indexed.get('publish-ios-beta')!, 'upload-testflight', [
    'upload-ios',
    '--manifest',
    '--beta-group-id',
  ]);
  const betaGroupEnvironment = mapping(
    indexed.get('publish-ios-beta')!.get('upload-testflight')!.env,
    'upload-testflight.env',
  );
  if (
    betaGroupEnvironment.APPSTORE_BETA_GROUP_ID !==
    '${{ vars.MDREADER_APPSTORE_BETA_GROUP_ID }}'
  ) {
    throw new Error(
      'TestFlight publisher must use the protected reviewed beta-group variable',
    );
  }
  requireRun(indexed.get('publish-android-beta')!, 'inspect-android', [
    'inspect-android',
    '--snapshot',
    '--manifest',
    '--closed-track "$GOOGLE_CLOSED_TRACK"',
  ]);
  const playTrackEnvironment = mapping(
    indexed.get('publish-android-beta')!.get('inspect-android')!.env,
    'inspect-android.env',
  );
  exactKeys(playTrackEnvironment, ['GOOGLE_CLOSED_TRACK'], 'inspect-android.env');
  if (
    playTrackEnvironment.GOOGLE_CLOSED_TRACK !==
    '${{ vars.MDREADER_GOOGLE_PLAY_CLOSED_TRACK }}'
  ) {
    throw new Error(
      'Google Play publisher must use the protected reviewed closed track',
    );
  }
  requireRun(indexed.get('publish-android-beta')!, 'upload-google-play', [
    'upload-android',
    '--manifest',
  ]);
  if (
    ['--closed-track', '--version-code', '--release-name'].some((option) =>
      scalar(
      indexed.get('publish-android-beta')!.get('upload-google-play')!.run,
      'upload-google-play.run',
      ).includes(option),
    )
  ) {
    throw new Error(
      'Google Play upload must derive its track and release identity from the manifest',
    );
  }

  const source = serialized(root);
  for (const forbidden of [
    'dev.mdreader.app.e2e',
    'e2e-build',
    'direct-updater',
    'DATABASE_URL',
    'POSTGRES_URL',
    'SNOWFLAKE',
    'WAREHOUSE_PASSWORD',
    '--artifact-path',
  ]) {
    if (source.includes(forbidden)) {
      throw new Error(`workflow contains forbidden mobile authority ${forbidden}`);
    }
  }
  if (/(^|\s)--debug(?:\s|$)|(^|\s)--apk(?:\s|$)/.test(source)) {
    throw new Error('workflow contains a debug or APK build');
  }
}
