import path from 'node:path';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

// Tauri serves the frontend from a fixed port and expects the build in dist/.
export default defineConfig({
  plugins: [react(), tailwindcss()],
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
    sourcemap: true,
  },
});
