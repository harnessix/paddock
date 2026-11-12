import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    include: ['test/**/*.test.ts', 'test/**/*.test.tsx'],
    // Only catches a hang: each wait in a test ends on its event, and a
    // busy CI runner can make one test take far longer than on a desk.
    testTimeout: 180_000,
    // The PTY test starts the built program; the others render in memory.
    pool: 'forks',
  },
});
