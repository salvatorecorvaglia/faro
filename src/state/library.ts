import { create } from 'zustand';

import * as ipc from '@/ipc';
import type { HistoryEntry, SavedQuery } from '@/ipc/types';

/**
 * Saved queries and query history.
 *
 * Both are small enough to keep wholly in memory and re-fetch on change, which
 * avoids the cache-invalidation problems that come with incremental updates.
 * History searching goes back to SQL rather than filtering locally, because the
 * store holds far more rows than the panel ever loads.
 */
interface LibraryState {
  saved: SavedQuery[];
  history: HistoryEntry[];
  historySearch: string;
  loading: boolean;
  /**
   * Set when a *write* fails. Reads stay silent (see `refreshSaved`), but a
   * delete or a history wipe that quietly did nothing is a different matter —
   * the user believes it happened.
   */
  error: string | null;

  refreshSaved: () => Promise<void>;
  refreshHistory: (search?: string) => Promise<void>;
  save: (query: Omit<SavedQuery, 'createdAt' | 'updatedAt'>) => Promise<SavedQuery>;
  remove: (id: string) => Promise<void>;
  clearHistory: () => Promise<void>;
  deleteHistoryEntry: (id: number) => Promise<void>;
  clearError: () => void;
}

export const useLibrary = create<LibraryState>((set, get) => ({
  saved: [],
  history: [],
  historySearch: '',
  loading: false,
  error: null,

  refreshSaved: async () => {
    try {
      set({ saved: await ipc.listSavedQueries() });
    } catch {
      // The library is an accessory to the editor; a failure to load it should
      // not put an error banner over the user's actual work.
    }
  },

  refreshHistory: async (search) => {
    const term = search ?? get().historySearch;
    set({ loading: true, historySearch: term });
    try {
      set({ history: await ipc.listHistory(term || undefined) });
    } catch {
      // As above.
    } finally {
      set({ loading: false });
    }
  },

  // Rethrows as well as recording: the save dialog awaits this to decide
  // whether to close, and closing on a failure would discard the user's query.
  save: async (query) => {
    set({ error: null });
    try {
      const saved = await ipc.saveQuery(query);
      await get().refreshSaved();
      return saved;
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
      throw e;
    }
  },

  remove: async (id) => {
    set({ error: null });
    try {
      await ipc.deleteSavedQuery(id);
      await get().refreshSaved();
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
    }
  },

  clearHistory: async () => {
    set({ error: null });
    try {
      await ipc.clearHistory();
      await get().refreshHistory();
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
    }
  },

  deleteHistoryEntry: async (id) => {
    set({ error: null });
    try {
      await ipc.deleteHistoryEntry(id);
      set((s) => ({ history: s.history.filter((h) => h.id !== id) }));
    } catch (e) {
      set({ error: ipc.errorMessage(e) });
    }
  },

  clearError: () => set({ error: null }),
}));

/** Group saved queries by folder, preserving the backend's ordering. */
export function groupByFolder(
  saved: SavedQuery[],
): { folder: string | null; queries: SavedQuery[] }[] {
  const groups: { folder: string | null; queries: SavedQuery[] }[] = [];
  for (const q of saved) {
    const folder = q.folder ?? null;
    const last = groups[groups.length - 1];
    if (last && last.folder === folder) last.queries.push(q);
    else groups.push({ folder, queries: [q] });
  }
  return groups;
}

/** Every distinct folder name, for the save dialog's suggestions. */
export function folderNames(saved: SavedQuery[]): string[] {
  return [...new Set(saved.map((q) => q.folder).filter((f): f is string => !!f))].sort();
}
