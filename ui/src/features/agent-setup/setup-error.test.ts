import { expect, test } from 'vitest';
import { agentSetupError } from './setup-error';
const safe = [
  'Agent configuration is too large for automatic setup.',
  'Agent integration conflicts with existing settings. Existing files were kept.',
  'Agent configuration could not be read. Existing files were kept.',
  'Agent setup is busy. Try again.',
  'This agent version or platform does not support the integration.',
  'Agent configuration could not be updated. Existing files were kept.',
];
test.each(safe)('safe native string and Invalid wrapper retain the useful message: %s', message => {
  expect(agentSetupError(message)).toBe(message);
  expect(agentSetupError(`Invalid request: ${message}`)).toBe(message);
  expect(agentSetupError(new Error(message))).toBe(message);
});
test.each(['I/O: /private/config token=secret', 'toString', '__proto__', '', null, { message: 'private detail' }])('unknown errors never expose details or inherited object members: %j', cause => {
  expect(agentSetupError(cause)).toBe('Agent setup could not be completed. Existing files were kept. Refresh status and try again.');
});
test('legacy conflict rejection uses agent setup language', () => {
  expect(agentSetupError('Document revision changed')).toBe('Agent integration conflicts with existing settings. Existing files were kept.');
});
