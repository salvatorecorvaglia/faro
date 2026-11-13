// Classic inline script: runs before application modules and their dependencies.
// Keep this independent of imports so syntax/load errors in the app are reportable.
(() => {
  let reported = false;
  const report = (error) => {
    if (reported) return;
    const bridge = window.__TAURI_INTERNALS__;
    if (!bridge) return;
    reported = true;
    void bridge
      .invoke('smoke_report', {
        result: {
          ok: false,
          phase: 'frontend bootstrap',
          error: String(error),
          url: location.href,
          userAgent: navigator.userAgent,
          readyState: document.readyState,
        },
      })
      .catch(() => {
        reported = false;
      });
  };
  window.addEventListener(
    'error',
    (event) => {
      const target = event.target;
      report(event.error || event.message || `Resource failed: ${target?.src || target?.href}`);
    },
    true,
  );
  window.addEventListener('unhandledrejection', (event) => report(event.reason));
  setTimeout(() => {
    if (!window.__FARO_SMOKE__) report('Desktop bridge did not start within 45 seconds');
  }, 45_000);
})();
