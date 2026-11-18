/**
 * Startup update-check policy, shared by every desktop app.
 *
 * One silent check, five seconds after startup: late enough not to compete
 * with first paint or database migrations, early enough that an available
 * update is known while the user is still getting oriented.
 *
 * The check is opt-out through `<APP>_DISABLE_UPDATE_CHECK=1`. In the Tauri
 * apps the variable is read at build time (see `envPrefix` in
 * `vite.config.ts`), because the webview cannot read the runtime environment.
 */
export const STARTUP_UPDATE_CHECK_DELAY_MS = 5_000;

/**
 * Schedule the automatic update check.
 *
 * The callback owns its failure handling — an automatic check must stay quiet,
 * so callers log rather than surface an offline error.
 *
 * Returns a cancel function so React effects can clear a pending timer.
 */
export function scheduleStartupUpdateCheck(
  check: () => unknown,
  { disabled = false }: { disabled?: boolean } = {},
): () => void {
  if (disabled) return () => {};

  const timer = setTimeout(() => {
    void check();
  }, STARTUP_UPDATE_CHECK_DELAY_MS);

  // Node keeps the process alive for a pending timer; browsers return a number.
  const unref = (timer as unknown as { unref?: () => void }).unref;
  if (typeof unref === 'function') unref.call(timer);

  return () => clearTimeout(timer);
}
