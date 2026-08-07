import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { create } from 'zustand';

export type UpdaterStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error';

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
      console.log('[Updater] Skipping update check in development mode.');
      return;
    }

    try {
      set({ status: 'checking', error: null });
      console.log('[Updater] Checking for updates...');
      const update = await check();

      if (update?.available) {
        console.log(`[Updater] Update v${update.version} is available.`);
        set({
          status: 'available',
          version: update.version,
          updateRef: update,
          dismissed: false,
        });
      } else {
        console.log('[Updater] No updates available.');
        set({ status: 'idle', updateRef: null });
      }
    } catch (err) {
      console.error('[Updater] Error checking for updates:', err);
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
      console.error('[Updater] Failed to download/install update:', err);
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
      console.error('[Updater] Failed to relaunch app:', err);
      set({
        status: 'error',
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  dismiss: () => set({ dismissed: true }),
}));
