import { readFileSync } from 'node:fs';
import path from 'node:path';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// Tauri serves the frontend from a fixed port and expects the build in dist/.
export default defineConfig(({ mode }) => ({
  // Expose FARO_DISABLE_UPDATE_CHECK to the webview. Tauri has no runtime
  // environment to read, so the update-check opt-out is set at build time,
  // with the same name the Electron apps read at launch.
  envPrefix: ['VITE_', 'FARO_'],
  plugins: [
    react(),
    tailwindcss(),
    ...(mode === 'desktop-smoke'
      ? [
          {
            name: 'native-smoke-bootstrap',
            transformIndexHtml() {
              return [
                {
                  tag: 'script',
                  children: readFileSync(
                    new URL('./tests/native/bootstrap.js', import.meta.url),
                    'utf8',
                  ),
                  injectTo: 'head-prepend' as const,
                },
              ];
            },
          },
        ]
      : []),
  ],
  resolve: {
    // import.meta.dirname rather than __dirname: Vite's native config loader
    // does not provide the CJS globals.
    alias: { '@': path.resolve(import.meta.dirname, 'src') },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    target: 'es2022',
    // No source maps in the build that ships. They were bundled into every
    // installer — added size, and nothing in the app or the release pipeline
    // reads them. `vite` dev serves maps regardless, so debugging is
    // unaffected; set this to `true` for a one-off build that needs them.
    sourcemap: false,
  },
}));
