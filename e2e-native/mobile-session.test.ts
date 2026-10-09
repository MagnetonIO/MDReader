import assert from 'node:assert/strict';
import test from 'node:test';
import { assertOwnedMobileSession } from './mobile-session.ts';

const device = '11111111-2222-4333-8444-555555555555';
test('accepts the actual XCUITest unprefixed response and namespaced response', () => {
  assertOwnedMobileSession({ bundleId: 'dev.mdreader.app.e2e', udid: device }, 'ios', device);
  assertOwnedMobileSession({ 'appium:bundleId': 'dev.mdreader.app.e2e', 'appium:udid': device }, 'ios', device);
  assertOwnedMobileSession({ appPackage: 'dev.mdreader.app.e2e', udid: 'emulator-5554' }, 'android', 'emulator-5554');
});
test('refuses missing identity, production identity and conflicting aliases', () => {
  for (const value of [{ udid: device }, { bundleId: 'dev.mdreader.app', udid: device },
    { 'appium:bundleId': 'dev.mdreader.app.e2e', bundleId: 'dev.mdreader.app', udid: device }])
    assert.throws(() => assertOwnedMobileSession(value, 'ios', device), /non-E2E/);
});
test('refuses missing, changed, conflicting and physical devices', () => {
  for (const value of [{}, { udid: '66666666-2222-4333-8444-555555555555' },
    { 'appium:udid': device, udid: '66666666-2222-4333-8444-555555555555' },
    { udid: 'physical-device' }])
    assert.throws(() => assertOwnedMobileSession({ bundleId: 'dev.mdreader.app.e2e', ...value }, 'ios', device));
});
