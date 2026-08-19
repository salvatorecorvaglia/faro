import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useState } from 'react';

import * as ipc from '@/ipc';

/**
 * The busy/error/done triad every dialog that fires one backend-writing
 * action needs: BackupDialog, RestoreDialog, ExportDialog and ImportDialog
 * each reimplemented the same try/catch/finally shape independently.
 *
 * Deliberately does not decide *when* to start — a caller that needs to do
 * something first (choose a save path, confirm a destructive action) does
 * that itself and only calls `run` once it is committed, so `busy` reflects
 * just the backend work, not a native file picker sitting on top of it.
 */
export function useAsyncAction() {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const reset = useCallback(() => {
    setError(null);
    setDone(null);
  }, []);

  /**
   * Run `action`, tracking busy/error/done around it.
   *
   * `action` returns the success message to show, or nothing for a dialog
   * that renders its own success UI instead of a one-line message.
   */
  // biome-ignore lint/suspicious/noConfusingVoidType: an action may fall off the end without an explicit return, which infers as void, not undefined.
  const run = useCallback(async (action: () => Promise<string | void>) => {
    setError(null);
    setDone(null);
    setBusy(true);
    try {
      const message = await action();
      if (message) setDone(message);
      return true;
    } catch (e) {
      setError(ipc.errorMessage(e));
      return false;
    } finally {
      setBusy(false);
    }
  }, []);

  return { busy, error, done, setError, setDone, reset, run };
}

/**
 * Subscribe to a Tauri progress event while `active`, so a backup or restore
 * that takes long enough to look like a hang shows what it is doing instead.
 */
export function useBackendProgress<T>(event: string, active: boolean) {
  const [progress, setProgress] = useState<T | null>(null);

  useEffect(() => {
    if (!active) {
      setProgress(null);
      return;
    }
    const unlisten = listen<T>(event, (e) => setProgress(e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, [event, active]);

  return progress;
}
