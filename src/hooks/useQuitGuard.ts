import { listen } from '@tauri-apps/api/event';
import { useEffect } from 'react';
import * as ipc from '@/ipc';
import { useConnections } from '@/state/connections';
import { confirmExit } from '@/state/exit';

export function useQuitGuard(): void {
  useEffect(() => {
    let disposed = false;
    let pending = false;
    const subscription = listen<boolean>('faro://quit-requested', async (event) => {
      if (disposed || pending) return;
      pending = true;
      try {
        if (await confirmExit(event.payload)) await ipc.approveExit();
      } catch (error) {
        useConnections.setState({ error: ipc.errorMessage(error) });
      } finally {
        pending = false;
      }
    });
    void subscription
      .then(async () => {
        if (!disposed) await ipc.setExitGuard(true);
      })
      .catch((error) => useConnections.setState({ error: ipc.errorMessage(error) }));
    return () => {
      disposed = true;
      void subscription.then((unlisten) => unlisten()).catch(() => {});
      void ipc.setExitGuard(false).catch(() => {});
    };
  }, []);
}
