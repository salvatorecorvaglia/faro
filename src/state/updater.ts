import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { create } from 'zustand';

export type UpdaterStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error';

// Development-only tracing. Every call site below used to reach `console.*`
// directly, gated only by the separate check that skips update checks
// entirely in dev — `downloadAndInstall` and `restartApp` have no such
// check, so their logging shipped to every user's console unconditionally.
const log = (...args: unknown[]) => {
  if (import.meta.env.DEV) console.log(...args);
};
const logError = (...args: unknown[]) => {
  if (import.meta.env.DEV) console.error(...args);
};

interface UpdaterState {
  status: UpdaterStatus;
  version: string | null;
  progress: number;
  error: string | null;
  updateRef: Update | null;
  dismissed: boolean;

  checkForUpdates: () => Promise<void>;
  downloadAndInstall: () => Promise<void>;
  restartApp: () => Promise<void>;
  dismiss: () => void;
}

export const useUpdaterStore = create<UpdaterState>((set, get) => ({
  status: 'idle',
  version: null,
  progress: 0,
  error: null,
  updateRef: null,
  dismissed: false,

  checkForUpdates: async () => {
    // Only run in production builds
    if (!import.meta.env.PROD) {
      log('[Updater] Skipping update check in development mode.');
      return;
    }

    try {
      set({ status: 'checking', error: null });
      log('[Updater] Checking for updates...');
      const update = await check();

      if (update?.available) {
        log(`[Updater] Update v${update.version} is available.`);
        set({
          status: 'available',
          version: update.version,
          updateRef: update,
          dismissed: false,
        });
      } else {
        log('[Updater] No updates available.');
        set({ status: 'idle', updateRef: null });
      }
    } catch (err) {
      logError('[Updater] Error checking for updates:', err);
      set({
        status: 'idle',
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  downloadAndInstall: async () => {
    const { updateRef } = get();
    if (!updateRef) return;

    try {
      set({ status: 'downloading', progress: 0, error: null });

      let downloaded = 0;
      let contentLength = 0;

      await updateRef.downloadAndInstall((event) => {
        switch (event.event) {
          case 'Started':
            contentLength = event.data.contentLength ?? 0;
            break;
          case 'Progress': {
            downloaded += event.data.chunkLength;
            const percent = contentLength ? Math.round((downloaded / contentLength) * 100) : 0;
            set({ progress: percent });
            break;
          }
          case 'Finished':
            break;
        }
      });

      set({ status: 'ready', progress: 100 });
    } catch (err) {
      logError('[Updater] Failed to download/install update:', err);
      set({
        status: 'error',
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  restartApp: async () => {
    try {
      await relaunch();
    } catch (err) {
      logError('[Updater] Failed to relaunch app:', err);
      set({
        status: 'error',
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  dismiss: () => set({ dismissed: true }),
}));
