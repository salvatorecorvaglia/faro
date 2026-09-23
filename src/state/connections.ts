import { create } from 'zustand';

import * as ipc from '@/ipc';
import type { ConnectionConfig, ConnectionStatus } from '@/ipc/types';
import { useSchemaCache } from './schemaCache';

interface ConnectionState {
  items: ConnectionStatus[];
  loading: boolean;
  /** Ids with a connect attempt in flight, so the row can show a spinner. */
  connecting: Set<string>;
  error: string | null;
  keychainOk: boolean;

  refresh: () => Promise<void>;
  connect: (id: string) => Promise<boolean>;
  disconnect: (id: string) => Promise<void>;
  save: (config: ConnectionConfig, password?: string) => Promise<ConnectionConfig>;
  remove: (id: string) => Promise<void>;
  clearError: () => void;
}

export const useConnections = create<ConnectionState>((set, get) => ({
  items: [],
  loading: true,
  connecting: new Set(),
  error: null,
  keychainOk: true,

  refresh: async () => {
    try {
      const [items, keychainOk] = await Promise.all([
        ipc.listConnectionStatus(),
        ipc.keychainAvailable(),
      ]);
      set({ items, keychainOk, loading: false });
    } catch (e) {
      set({ error: ipc.errorMessage(e), loading: false });
    }
  },

  connect: async (id) => {
    set((s) => ({ connecting: new Set(s.connecting).add(id), error: null }));
    try {
      await ipc.connect(id);
      await get().refresh();
      return true;
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
      return false;
    } finally {
      set((s) => {
        const next = new Set(s.connecting);
        next.delete(id);
        return { connecting: next };
      });
    }
  },

  // The three below report failures into `error` rather than letting the
  // promise reject. Their call sites in Sidebar are plain `onClick` handlers
  // with no catch, so a rejection became an unhandled promise rejection and the
  // user saw nothing — a delete that silently did not happen is the worst case.

  disconnect: async (id) => {
    try {
      await ipc.disconnect(id);
      // Drop the cached schema: by the time this connection is reopened the
      // database may have changed, and stale autocomplete is worse than none.
      useSchemaCache.getState().invalidate(id);
      await get().refresh();
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
    }
  },

  // Rethrows as well as recording: the connection dialog awaits this to decide
  // whether to close, and closing on a failed save would discard the user's
  // input along with the error.
  save: async (config, password) => {
    try {
      const saved = await ipc.saveConnection(config, password);
      await get().refresh();
      return saved;
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
      throw e;
    }
  },

  remove: async (id) => {
    try {
      await ipc.deleteConnection(id);
      await get().refresh();
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
    }
  },

  clearError: () => set({ error: null }),
}));
