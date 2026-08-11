import path from 'node:path';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

export default defineConfig({
  // The React plugin is what compiles JSX in `.test.tsx` files.
  plugins: [react()],
  resolve: {
    alias: { '@': path.resolve(import.meta.dirname, 'src') },
  },
  test: {
    // jsdom rather than node: component tests need a DOM, and the previous
    // `node` environment plus a `.test.ts`-only glob meant a `.test.tsx` file
    // could not even be collected — so there were no component tests at all.
    environment: 'jsdom',
    include: ['src/**/*.test.{ts,tsx}'],
    setupFiles: ['src/test/setup.ts'],
    // Each file gets a fresh module registry so a zustand store mutated by one
    // test cannot leak into the next. The stores are module-level singletons,
    // which is convenient in the app and a shared-state hazard in tests.
    isolate: true,
    restoreMocks: true,
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      include: ['src/**/*.{ts,tsx}'],
      exclude: ['src/**/*.test.{ts,tsx}', 'src/test/**', 'src/vite-env.d.ts', 'src/main.tsx'],
    },
  },
});
