import { validateAndroidApk, validateIosApp } from './capabilities.ts';

const [platform, artifact, sourceCommit] = process.argv.slice(2);

if (platform === 'ios') {
  process.stdout.write(`${validateIosApp(artifact, sourceCommit)}\n`);
} else if (platform === 'android') {
  process.stdout.write(`${validateAndroidApk(artifact, sourceCommit)}\n`);
} else {
  throw new Error(
    'usage: validate-artifact.ts <ios|android> <absolute artifact path> [source commit]',
  );
}
