import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';
import { mobileDeviceUdid } from './capabilities.ts';

/** Appium returns extension capabilities both with and without their request
 * namespace. Validate the server response, refusing missing or conflicting
 * aliases; the desired capabilities are never evidence of the actual session. */
export function assertOwnedMobileSession(
  capabilities: Record<string, unknown>,
  platform: 'ios' | 'android',
  expectedUdid: string,
): void {
  const identityKey = platform === 'ios' ? 'bundleId' : 'appPackage';
  const identities = [capabilities[`appium:${identityKey}`], capabilities[identityKey]]
    .filter((value) => value !== undefined);
  if (!identities.length || identities.some((value) => value !== E2E_BUNDLE_IDENTIFIER))
    throw new Error('Refusing non-E2E application identity');
  const devices = [capabilities['appium:udid'], capabilities.udid]
    .filter((value) => value !== undefined);
  const expected = mobileDeviceUdid(platform, expectedUdid);
  if (!devices.length || devices.some((value) => typeof value !== 'string'
    || mobileDeviceUdid(platform, value) !== expected))
    throw new Error('Appium did not bind the explicitly selected owned mobile device');
}
