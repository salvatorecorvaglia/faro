import base from './playwright.config';
export default {
  ...base,
  outputDir: 'test-results-startup',
  testMatch: '**/startup.spec.ts',
  webServer: {
    command: 'pnpm exec vite preview --host 127.0.0.1 --port 1420',
    url: 'http://127.0.0.1:1420',
    reuseExistingServer: false,
  },
};
