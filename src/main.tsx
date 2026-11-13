import React from 'react';
import ReactDOM from 'react-dom/client';

import App from './App';
import './styles.css';

/**
 * Follow the OS light/dark setting.
 *
 * The `dark` class on <html> is what the CSS variables key off, and it is set
 * before React mounts so the first paint is already the right theme.
 */
const media = window.matchMedia('(prefers-color-scheme: dark)');
const applyTheme = (dark: boolean) => document.documentElement.classList.toggle('dark', dark);

applyTheme(media.matches);
media.addEventListener('change', (e) => applyTheme(e.matches));

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

// Removed from normal production bundles by Vite.
if (import.meta.env.MODE === 'desktop-smoke') {
  const reportFailure = (error: unknown) =>
    import('@tauri-apps/api/core').then(({ invoke }) =>
      invoke('smoke_report', {
        result: { ok: false, error: String(error), phase: 'frontend bootstrap' },
      }),
    );
  window.addEventListener('error', (event) => void reportFailure(event.error ?? event.message));
  window.addEventListener('unhandledrejection', (event) => void reportFailure(event.reason));
  void import('../tests/native/smoke.js').then((smoke) => smoke.start()).catch(reportFailure);
  void import('./test/desktopBridge').then((bridge) => bridge.start()).catch(reportFailure);
}
