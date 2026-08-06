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
