import { execFileSync } from 'node:child_process';
import { readFileSync, statSync } from 'node:fs';
import { resolve } from 'node:path';

import { parse } from 'yaml';
import { describe, expect, test } from 'vitest';

import { validateReleaseWorkflow } from '../workflow-contract';

const repositoryRoot = resolve(process.cwd(), '..', '..');
const RELEASE_NODE_VERSION = '24.18.0';
const workflowPath = resolve(
  repositoryRoot,
  '.github',
  'workflows',
  'release.yml',
);

function workflow(): Record<string, unknown> {
  return parse(readFileSync(workflowPath, 'utf8')) as Record<string, unknown>;
}

function clone<T>(value: T): T {
  return structuredClone(value);
}

function jobs(
  document: Record<string, unknown>,
): Record<string, Record<string, unknown>> {
  return document.jobs as Record<string, Record<string, unknown>>;
}

function steps(job: Record<string, unknown>): Array<Record<string, unknown>> {
  return job.steps as Array<Record<string, unknown>>;
}

function step(
  document: Record<string, unknown>,
  job: string,
  id: string,
): Record<string, unknown> {
  return steps(jobs(document)[job]!).find((entry) => entry.id === id)!;
}

describe('desktop release workflow contract', () => {
  test('reviewed release shell commands are executable and parse as scripts', () => {
    for (const script of [
      'verify-macos-signature.sh',
      'verify-macos-notarization.sh',
      'generate-direct-release-config.sh',
      'require-release-secrets.sh',
      'prepare-direct-release.sh',
      'bundle-direct-release.sh',
      'verify-direct-release.sh',
      'verify-credentialed-output.sh',
      'build-sealed-publisher.sh',
      'verify-linux-packages.sh',
      'verify-final-packages.sh',
      'inspect-sboms.sh',
      'stage-platform.sh',
      'validate-staged-platform.sh',
    ]) {
      const path = resolve(repositoryRoot, 'scripts', 'release', script);
      expect(statSync(path).mode & 0o111).not.toBe(0);
      expect(() => execFileSync('bash', ['-n', path])).not.toThrow();
    }
  });

  test('the shipped final verifier snapshots packages and uses only validated extraction', () => {
    const source = readFileSync(
      resolve(
        repositoryRoot,
        'scripts',
        'release',
        'verify-final-packages.sh',
      ),
      'utf8',
    );
    expect(source).toContain('snapshot_file');
    expect(source).toContain('cmp -s -- "$source" "$snapshot"');
    expect(source).toContain('safe-package-extract.mjs');
    expect(source).toContain('dpkg-deb --field "$DEB" Package');
    expect(source).toContain("rpm -qp --queryformat '%{NAME}'");
    expect(source).toContain("DEB_PACKAGE\" == 'mdreader'");
    expect(source).toContain("RPM_PACKAGE\" == 'mdreader'");
    expect(source).toContain('dpkg-deb --field "$DEB" Version');
    expect(source).toContain("rpm -qp --queryformat '%{VERSION}'");
    expect(source).toContain("rpm -qp --queryformat '%{RELEASE}'");
    expect(source).toContain('EXPECTED_VERSION');
    expect(source).not.toMatch(
      /\btar\s+-x|\b7z\s+x\s+-y|\bcpio\s+-|\bdpkg-deb\s+-x|rpm2cpio.*\|/,
    );
  });

  test('establishes package identity before Linux smoke and never executes an AppImage to inspect it', () => {
    const document = workflow();
    const verifySteps = steps(jobs(document).verify!);
    const identity = verifySteps.findIndex(
      (entry) => entry.id === 'verify-final-package-binaries',
    );
    const smoke = verifySteps.findIndex(
      (entry) => entry.id === 'linux-verify-packages',
    );
    expect(identity).toBeGreaterThanOrEqual(0);
    expect(identity).toBeLessThan(smoke);
    expect(String(verifySteps[smoke]!.run)).toContain(
      'mdreader-final-package-binaries.json',
    );

    const source = readFileSync(
      resolve(
        repositoryRoot,
        'scripts',
        'release',
        'verify-linux-packages.sh',
      ),
      'utf8',
    );
    expect(source).toContain('identity evidence path is required');
    expect(source).toContain('safe-package-extract.mjs');
    expect(source).not.toContain('--appimage-extract');
    expect(source).not.toMatch(/^\s*"\$\{APPIMAGES\[0\]\}"/m);
  });

  test('seals the NSIS install and launch target into the compiled package contract', () => {
    const source = readFileSync(
      resolve(repositoryRoot, 'scripts', 'release', 'windows-minimum.nsh'),
      'utf8',
    );
    expect(source).toContain(
      '!if "${MAINBINARYNAME}" != "mdreader-app"',
    );
    expect(source).toContain('!if "${PRODUCTNAME}" != "MDReader"');
    expect(source).toContain(
      '!if "${BUNDLEID}" != "dev.mdreader.app"',
    );
    expect(source).toContain('mdreader-package-contract');
    expect(source).toContain('$INSTDIR\\mdreader-app.exe');
    expect(source).toContain('${VERSION}');
  });

  test('passes the exact release version through every final package verifier', () => {
    const document = workflow();
    expect(
      String(step(document, 'verify', 'verify-final-package-binaries').run),
    ).toContain('needs.prepare.outputs.version');
    expect(
      String(step(document, 'verify', 'windows-verify-signature').run),
    ).toContain('ExpectedVersion');
    expect(
      String(step(document, 'verify', 'linux-verify-packages').run),
    ).toContain('needs.prepare.outputs.version');
    const windowsVerifier = readFileSync(
      resolve(
        repositoryRoot,
        'scripts',
        'release',
        'verify-windows-signature.ps1',
      ),
      'utf8',
    );
    expect(windowsVerifier).toContain(
      '$version.ProductVersion -cne $ExpectedVersion',
    );
    expect(windowsVerifier).toContain(
      '$version.FileVersion -cne $ExpectedVersion',
    );
  });

  test('every inline Bash body parses after the workflow split', () => {
    for (const job of Object.values(jobs(workflow()))) {
      for (const entry of steps(job)) {
        if (typeof entry.run !== 'string' || entry.shell === 'pwsh') continue;
        const run = entry.run;
        expect(() =>
          execFileSync('bash', ['-n'], {
            input: run,
            stdio: ['pipe', 'ignore', 'pipe'],
          }),
        ).not.toThrow();
      }
    }
  });

  test('the tracked split release workflow satisfies the exact contract', () => {
    expect(() => validateReleaseWorkflow(workflow())).not.toThrow();
  });

  test('pins signing credentials to independently reviewed publisher identities', () => {
    const document = workflow();
    // The reviewed identity is read and validated once, in prepare, before any
    // credential exists; the credentialed jobs consume that sealed output.
    expect(step(document, 'prepare', 'authorize-apple-team').env).toMatchObject({
      EXPECTED_APPLE_TEAM_ID: '${{ vars.MDREADER_APPLE_TEAM_ID }}',
    });
    expect(step(document, 'bundle-sign', 'require-release-secrets-macos').env).toMatchObject({
      EXPECTED_APPLE_TEAM_ID: '${{ needs.prepare.outputs.apple_team_id }}',
    });
    expect(step(document, 'bundle-sign', 'inspect-windows-cert').env).toMatchObject({
      EXPECTED_WINDOWS_CERTIFICATE_THUMBPRINT:
        '${{ vars.MDREADER_WINDOWS_CERTIFICATE_THUMBPRINT }}',
    });
    expect(step(document, 'verify', 'verify-final-package-binaries').env).toEqual({
      EXPECTED_APPLE_TEAM_ID: '${{ needs.prepare.outputs.apple_team_id }}',
      EXPECTED_WINDOWS_CERTIFICATE_THUMBPRINT:
        '${{ vars.MDREADER_WINDOWS_CERTIFICATE_THUMBPRINT }}',
    });
    expect(step(document, 'verify', 'windows-verify-signature').env).toEqual({
      EXPECTED_WINDOWS_CERTIFICATE_THUMBPRINT:
        '${{ vars.MDREADER_WINDOWS_CERTIFICATE_THUMBPRINT }}',
    });
    expect(
      String(step(document, 'bundle-sign', 'require-release-secrets-macos').run),
    ).toContain('APPLE_TEAM_ID" == "$EXPECTED_APPLE_TEAM_ID');
    expect(
      String(step(document, 'bundle-sign', 'inspect-windows-cert').run),
    ).toContain(
      '$certificate.Thumbprint.ToUpperInvariant() -ne $env:EXPECTED_WINDOWS_CERTIFICATE_THUMBPRINT',
    );
  });

  test('pins Node 24 LTS in every release job, including publication', () => {
    const releaseJobs = jobs(workflow());
    for (const [jobName, job] of Object.entries(releaseJobs)) {
      const setup = steps(job).filter((entry) =>
        String(entry.uses ?? '').startsWith('actions/setup-node@'),
      );
      expect(setup, `${jobName} must install one reviewed Node runtime`).toHaveLength(1);
      expect(setup[0]!.with).toMatchObject({
        'node-version': RELEASE_NODE_VERSION,
      });
      expect(setup[0]!.name).toMatch(/Node 24 LTS/);
    }
  });

  test('establishes exact Node 24.18.0 before every other release step', () => {
    for (const [jobName, job] of Object.entries(jobs(workflow()))) {
      expect(
        steps(job)[0],
        `${jobName} must have no direct or action-mediated Node execution before setup`,
      ).toMatchObject({
        id: `setup-node-${jobName === 'build-unsigned' ? 'build' : jobName === 'bundle-sign' ? 'bundle' : jobName}`,
        uses:
          'actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020',
        with: {
          'node-version': RELEASE_NODE_VERSION,
        },
      });
    }
  });

  test.each([
    ['prepare', 'node -e "process.exit(0)"'],
    ['build-unsigned', 'bash scripts/release/build-sealed-publisher.sh out.mjs'],
    ['bundle-sign', 'pnpm --version'],
    ['verify', 'npx tsx --version'],
    ['aggregate', 'corepack enable'],
    ['publish', 'node target/release-publish/publish-sealed-plan.mjs'],
  ])('rejects direct or indirect Node execution before setup in %s', (jobName, run) => {
    const document = clone(workflow());
    steps(jobs(document)[jobName]!).unshift({
      name: 'pre-setup execution',
      id: 'pre-setup-execution',
      run,
    });
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /exact Node 24\.18\.0.*before every other release step/i,
    );
  });

  test('rejects an action-mediated Node invocation before exact setup', () => {
    const document = clone(workflow());
    steps(jobs(document).prepare!).unshift({
      name: 'Checkout before runtime setup',
      id: 'pre-setup-action',
      uses: 'actions/checkout@11d5960a326750d5838078e36cf38b85af677262',
      with: { 'persist-credentials': false },
    });
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /exact Node 24\.18\.0.*before every other release step/i,
    );
  });

  test('rejects preloading code into the setup action Node process', () => {
    const document = clone(workflow());
    step(document, 'prepare', 'setup-node-prepare').env = {
      NODE_OPTIONS: '--require ./unreviewed-preload.cjs',
    };
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /exact Node 24\.18\.0.*before every other release step/i,
    );
  });

  test('the workflow validator rejects Node 22 in every release job', () => {
    for (const jobName of Object.keys(jobs(workflow()))) {
      const document = clone(workflow());
      const setup = steps(jobs(document)[jobName]!).find((entry) =>
        String(entry.uses ?? '').startsWith('actions/setup-node@'),
      );
      expect(setup, `${jobName} must expose its reviewed Node step`).toBeDefined();
      setup!.with = {
        ...(setup!.with as Record<string, unknown>),
        'node-version': '22.13.1',
      };
      expect(
        () => validateReleaseWorkflow(document),
        `${jobName} must reject Node 22`,
      ).toThrow(/Node 24(?: LTS|\.18\.0)|immutable reviewed action/i);
    }
  });

  test('the workspace runtime contract rejects Node 22', () => {
    const packageMetadata = JSON.parse(
      readFileSync(resolve(repositoryRoot, 'package.json'), 'utf8'),
    ) as { engines?: { node?: string } };
    expect(packageMetadata.engines?.node).toBe('^24.0.0');
    expect(packageMetadata.engines?.node).not.toMatch(/22/);
  });

  test('documents the un-inventoried runner OS-tool trust boundary explicitly', () => {
    const source = readFileSync(workflowPath, 'utf8');
    expect(source).toContain('OS/tool trust boundary');
    expect(source).toContain('GitHub runner-image native packaging');
    expect(source).toContain('outside the byte inventory');
  });

  test('rejects every unreviewed top-level job', () => {
    const document = clone(workflow());
    jobs(document).shadow = {
      'runs-on': 'ubuntu-24.04',
      steps: [{ id: 'shadow', run: 'echo unreviewed' }],
    };
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /top-level release jobs/,
    );
  });

  test.each([
    'prepare',
    'build-unsigned',
    'bundle-sign',
    'verify',
    'aggregate',
    'publish',
  ])('rejects an extra step in %s', (jobName) => {
    const document = clone(workflow());
    steps(jobs(document)[jobName]!).push({
      id: 'unreviewed',
      run: 'echo unreviewed',
    });
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /complete ordered step set/,
    );
  });

  test('keeps only bundle signing and publication behind protected environments', () => {
    const releaseJobs = jobs(workflow());
    const bundleMatrix = (
      (
        releaseJobs['bundle-sign']!.strategy as Record<string, unknown>
      ).matrix as Record<string, unknown>
    ).include as Array<Record<string, unknown>>;
    expect(releaseJobs['build-unsigned']!.environment).toBeUndefined();
    expect(releaseJobs.verify!.environment).toBeUndefined();
    expect(releaseJobs['bundle-sign']!.environment).toBe(
      '${{ matrix.environment }}',
    );
    expect(bundleMatrix.map((entry) => entry.environment)).toEqual([
      'mdreader-v1-release-20260726-macos',
      'mdreader-v1-release-20260726-windows',
      'mdreader-v1-release-20260726-linux',
    ]);
    expect(releaseJobs.publish!.environment).toBe(
      'mdreader-v1-release-20260726-publication',
    );
  });

  test('rejects protected environment names referenced by historical workflows', () => {
    const document = clone(workflow());
    const bundleMatrix = (
      (
        jobs(document)['bundle-sign']!.strategy as Record<string, unknown>
      ).matrix as Record<string, unknown>
    ).include as Array<Record<string, unknown>>;
    bundleMatrix[0]!.environment = 'mdreader-production-macos';
    jobs(document).publish!.environment =
      'mdreader-production-publication';

    expect(() => validateReleaseWorkflow(document)).toThrow(
      /release matrix|protected environment/i,
    );
  });

  test('rejects weakening exact main-tip authority back to ancestry', () => {
    const document = clone(workflow());
    const authorization = step(
      document,
      'prepare',
      'authorize-release-source',
    );
    authorization.run = String(authorization.run).replace(
      '[[ "$SOURCE_COMMIT" == "$MAIN_TIP" ]]',
      'git merge-base --is-ancestor "$SOURCE_COMMIT" "$MAIN_TIP"',
    );

    expect(() => validateReleaseWorkflow(document)).toThrow(
      /exact current main tip/i,
    );
  });

  test.each(['build-unsigned', 'bundle-sign', 'verify'])(
    'rejects a partial or mixed %s platform matrix',
    (jobName) => {
      const document = clone(workflow());
      const strategy = jobs(document)[jobName]!.strategy as Record<
        string,
        unknown
      >;
      const matrix = strategy.matrix as Record<string, unknown>;
      const include = matrix.include as Array<Record<string, unknown>>;
      include[1]!.target = 'universal-apple-darwin';
      expect(() => validateReleaseWorkflow(document)).toThrow(
        /release matrix/,
      );
    },
  );

  test('rejects publication before fresh uncredentialed verification', () => {
    const document = clone(workflow());
    jobs(document).aggregate!.needs = ['prepare', 'bundle-sign'];
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /dependency boundary/,
    );
  });

  test('rejects secret access in uncredentialed build and verification jobs', () => {
    for (const jobName of ['build-unsigned', 'verify']) {
      const document = clone(workflow());
      steps(jobs(document)[jobName]!).find(
        (entry) => typeof entry.run === 'string',
      )!.env = { PRIVATE: '${{ secrets.EXTRA_PRIVATE_KEY }}' };
      expect(() => validateReleaseWorkflow(document)).toThrow(
        /secret allowlist|uncredentialed/,
      );
    }
  });

  test('rejects extra or rebound secrets in a reviewed bundle credential step', () => {
    const document = clone(workflow());
    const credential = step(
      document,
      'bundle-sign',
      'require-release-secrets-linux',
    );
    credential.env = {
      ...(credential.env as Record<string, unknown>),
      EXTRA: '${{ secrets.EXTRA_SIGNING_SECRET }}',
    };
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /signing environment/,
    );
  });

  test.each([
    '${{ secrets }}',
    '${{ toJSON(secrets) }}',
    "${{ secrets['EXTRA_SIGNING_SECRET'] }}",
    '${{ SeCrEtS.EXTRA_SIGNING_SECRET }}',
  ])('rejects computed or whole secret context access: %s', (value) => {
    const document = clone(workflow());
    step(document, 'prepare', 'authorize-release-source').name = value;
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /secret allowlist/,
    );
  });

  test('requires support and updater-key sealing before package installation', () => {
    const document = workflow();
    const prepare = steps(jobs(document).prepare!);
    const seal = prepare.findIndex(
      (entry) => entry.id === 'seal-release-support',
    );
    const install = prepare.findIndex(
      (entry) => entry.id === 'install-dependencies-prepare',
    );
    expect(seal).toBeGreaterThanOrEqual(0);
    expect(seal).toBeLessThan(install);
    expect(String(prepare[seal]!.run)).toContain(
      'updater_public_key_sha256',
    );
  });

  test('passes the exact public OAuth variables into the shipping Tauri build', () => {
    const document = workflow();
    expect(
      step(document, 'build-unsigned', 'prepare-direct-release').env,
    ).toEqual({
      MDREADER_GOOGLE_DRIVE_CLIENT_ID:
        '${{ vars.MDREADER_GOOGLE_DRIVE_CLIENT_ID }}',
      MDREADER_DROPBOX_CLIENT_ID:
        '${{ vars.MDREADER_DROPBOX_CLIENT_ID }}',
      MDREADER_DROPBOX_REDIRECT_URI:
        '${{ vars.MDREADER_DROPBOX_REDIRECT_URI }}',
      EXPECTED_GOOGLE_DRIVE_CLIENT_ID_SHA256:
        '${{ needs.prepare.outputs.google_drive_client_id_sha256 }}',
      EXPECTED_DROPBOX_CLIENT_ID_SHA256:
        '${{ needs.prepare.outputs.dropbox_client_id_sha256 }}',
      EXPECTED_DROPBOX_REDIRECT_URI_SHA256:
        '${{ needs.prepare.outputs.dropbox_redirect_uri_sha256 }}',
      MDREADER_RELEASE_SOURCE_COMMIT:
        '${{ needs.prepare.outputs.source_commit }}',
      MDREADER_MACOS_ORT_LIB_LOCATION:
        '${{ steps.prepare-macos-ort-runtime.outputs.ort_lib_location }}',
      MDREADER_MACOS_ORT_MANIFEST:
        '${{ steps.prepare-macos-ort-runtime.outputs.ort_manifest_path }}',
    });
  });

  test.each([
    'MDREADER_GOOGLE_DRIVE_CLIENT_ID',
    'MDREADER_DROPBOX_CLIENT_ID',
    'MDREADER_DROPBOX_REDIRECT_URI',
  ])('rejects removing %s from the shipping Tauri build', (name) => {
    const document = clone(workflow());
    const build = step(
      document,
      'build-unsigned',
      'prepare-direct-release',
    );
    delete (build.env as Record<string, unknown>)[name];

    expect(() => validateReleaseWorkflow(document)).toThrow(
      /public OAuth|shipping Tauri build/i,
    );
  });

  test('rejects rebinding sealed prepare outputs', () => {
    const document = clone(workflow());
    jobs(document).prepare!.outputs = {
      ...(jobs(document).prepare!.outputs as Record<string, unknown>),
      support_sha256: '${{ github.sha }}',
    };
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /prepare outputs/,
    );
  });

  test('rejects an unsigned handoff name without its manifest hash', () => {
    const document = clone(workflow());
    const upload = step(
      document,
      'build-unsigned',
      'upload-unsigned-handoff',
    );
    (upload.with as Record<string, unknown>).name =
      'mdreader-unsigned-${{ matrix.platform }}';
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /immutable reviewed action/,
    );
  });

  test('rejects removing the closed unsigned allowlist', () => {
    const document = clone(workflow());
    const seal = step(
      document,
      'build-unsigned',
      'seal-unsigned-handoff',
    );
    seal.run = String(seal.run).replace(
      'const sources = [',
      'const sources = undefined;',
    );
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /handoff.*closed|hash-addressed/i,
    );
  });

  test('rejects omitting the committed public OAuth authority from the unsigned handoff', () => {
    const document = clone(workflow());
    const seal = step(
      document,
      'build-unsigned',
      'seal-unsigned-handoff',
    );
    seal.run = String(seal.run).replaceAll(
      'public-oauth-authority.json',
      'unreviewed-oauth-authority.json',
    );
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /OAuth.*closed|hash-addressed/i,
    );
  });

  test('rejects checkout, package-manager build code, or wrappers in the credentialed job', () => {
    for (const mutation of [
      { uses: 'actions/checkout@' + 'a'.repeat(40) },
      { run: 'pnpm build' },
      { run: 'scripts/release/bundle-direct-release.sh' },
      { run: 'node node_modules/@tauri-apps/cli/tauri.js bundle' },
    ]) {
      const document = clone(workflow());
      const boundary = steps(jobs(document)['bundle-sign']!).findIndex(
        (entry) => entry.id === 'verify-credential-boundaries',
      );
      steps(jobs(document)['bundle-sign']!).splice(boundary + 1, 0, {
        id: 'injected',
        ...mutation,
      });
      expect(() => validateReleaseWorkflow(document)).toThrow(
        /ordered step set|credentialed|checkout-controlled/,
      );
    }
  });

  test('rejects weakening the closed handoff and runtime verifier', () => {
    const document = clone(workflow());
    const boundary = step(
      document,
      'bundle-sign',
      'verify-credential-boundaries',
    );
    boundary.run = String(boundary.run).replace(
      'Node runtime inventory mismatch',
      'node accepted',
    );
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /credential boundary/,
    );
  });

  test('rejects unauthenticated or direct nested runtime extraction', () => {
    const unauthenticated = clone(workflow());
    step(
      unauthenticated,
      'bundle-sign',
      'verify-release-extractor',
    ).run = 'echo extractor accepted';
    expect(() => validateReleaseWorkflow(unauthenticated)).toThrow(
      /extractor.*authenticated/i,
    );

    const direct = clone(workflow());
    const runtime = step(
      direct,
      'bundle-sign',
      'establish-tauri-runtime',
    );
    runtime.run = String(runtime.run).replaceAll(
      'node "$SAFE_PACKAGE_EXTRACTOR"',
      'tar -xzf',
    );
    expect(() => validateReleaseWorkflow(direct)).toThrow(
      /isolated Tauri and Node runtime|inventoried/i,
    );
  });

  test('rejects an archive producer that does not emit the closed POSIX ustar namespace', () => {
    const document = clone(workflow());
    const seal = step(
      document,
      'bundle-sign',
      'seal-credentialed-output',
    );
    seal.run = String(seal.run)
      .replace("'--format=ustar',", '')
      .replace(
        "'credentialed-output.json', 'release-config.json', 'bundle'",
        "'.'",
      );
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /closed POSIX ustar namespace/,
    );
  });

  test('rejects a credentialed-output producer that can seal multiply-linked files', () => {
    const document = clone(workflow());
    const seal = step(
      document,
      'bundle-sign',
      'seal-credentialed-output',
    );
    seal.run = String(seal.run).replace('metadata.nlink !== 1', 'false');
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /hard-linked credentialed output/,
    );
  });

  test('rejects a checkout path fallback in the credential boundary', () => {
    const document = clone(workflow());
    const boundary = step(
      document,
      'bundle-sign',
      'verify-credential-boundaries',
    );
    boundary.run = `${String(boundary.run)}\n# GITHUB_WORKSPACE fallback`;
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /checkout-controlled paths/,
    );
  });

  test.each([
    'bundle-direct-release-macos',
    'bundle-direct-release-windows',
    'bundle-direct-release-linux',
  ])('rejects %s disconnected from the inventoried runtime', (id) => {
    const document = clone(workflow());
    step(document, 'bundle-sign', id).run =
      'node node_modules/@tauri-apps/cli/tauri.js bundle';
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /isolated inventoried Tauri runtime|checkout-controlled/,
    );
  });

  test('rejects removal of the Windows NSIS hook from the final config verifier', () => {
    const document = clone(workflow());
    const verifier = step(
      document,
      'bundle-sign',
      'verify-credentialed-release-config',
    );
    verifier.run = String(verifier.run).replace(
      'windows?.nsis?.installerHooks',
      'windows?.ignored',
    );
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /NSIS hook|platform tuple/,
    );
  });

  test.each([
    ['macos-verify-signature', /macOS signature/],
    ['macos-verify-notarization', /notarization/],
    ['windows-verify-signature', /Windows signing/],
    ['linux-verify-packages', /Linux package/],
    [
      'verify-final-package-binaries',
      /final packages|reviewed MDReader executable/,
    ],
    ['verify-credentialed-output', /credentialed output/],
  ])('rejects a placeholder %s verifier', (id, error) => {
    const document = clone(workflow());
    step(document, 'verify', id).run = 'echo placeholder';
    expect(() => validateReleaseWorkflow(document)).toThrow(error);
  });

  test('rejects weakened platform verification conditions', () => {
    const document = clone(workflow());
    step(document, 'verify', 'macos-verify-notarization').if =
      '${{ false }}';
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /invalid condition/,
    );
  });

  test('requires a pinned immutable Syft action and version', () => {
    const mutable = clone(workflow());
    step(mutable, 'verify', 'install-syft').uses =
      'anchore/sbom-action/download-syft@v1';
    expect(() => validateReleaseWorkflow(mutable)).toThrow(
      /immutable reviewed action/,
    );

    const latest = clone(workflow());
    step(latest, 'verify', 'install-syft').with = {
      'syft-version': 'latest',
    };
    expect(() => validateReleaseWorkflow(latest)).toThrow(
      /immutable reviewed action/,
    );
  });

  test('pins every external action to an immutable full SHA', () => {
    const document = workflow();
    for (const job of Object.values(jobs(document))) {
      for (const entry of steps(job)) {
        if (typeof entry.uses !== 'string') continue;
        expect(entry.uses).toMatch(/^[^@\s]+@[0-9a-f]{40}$/);
      }
    }
  });

  test('rejects replacing any reviewed artifact action', () => {
    for (const [job, id] of [
      ['bundle-sign', 'download-unsigned-handoff'],
      ['verify', 'upload-staged-release'],
      ['aggregate', 'download-staged-release-macos'],
    ] as const) {
      const document = clone(workflow());
      step(document, job, id).uses =
        'actions/checkout@11d5960a326750d5838078e36cf38b85af677262';
      expect(() => validateReleaseWorkflow(document)).toThrow(
        /immutable reviewed action/,
      );
    }
  });

  test('rejects publication credentials outside the single reviewed step', () => {
    const document = clone(workflow());
    jobs(document).publish!.env = {
      TOKEN: '${{ secrets.EXTRA_RELEASE_SECRET }}',
    };
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /job metadata|publication credentials/,
    );
  });

  test('rejects extra or rebound R2 credentials', () => {
    const document = clone(workflow());
    const publisher = step(document, 'publish', 'publish-r2');
    publisher.env = {
      ...(publisher.env as Record<string, unknown>),
      EXTRA: '${{ secrets.EXTRA_RELEASE_SECRET }}',
    };
    expect(() => validateReleaseWorkflow(document)).toThrow(
      /publisher environment|secret allowlist/,
    );
  });

  test('serializes stable releases and refuses broad workflow permissions', () => {
    const concurrency = clone(workflow());
    concurrency.concurrency = {
      group: 'mdreader-${{ github.ref }}',
      'cancel-in-progress': false,
    };
    expect(() => validateReleaseWorkflow(concurrency)).toThrow(
      /stable-channel concurrency/,
    );

    const permissions = clone(workflow());
    permissions.permissions = { contents: 'write' };
    expect(() => validateReleaseWorkflow(permissions)).toThrow(
      /least privilege/,
    );
  });
});
