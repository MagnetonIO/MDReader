// Shared Vitest base config for all @mdreader packages.
// Packages import this and merge in their own `test.environment`.
export default {
  test: {
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    reporters: ['default'],
    clearMocks: true,
  },
};
