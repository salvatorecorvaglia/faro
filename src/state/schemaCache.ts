import { create } from 'zustand';

import * as ipc from '@/ipc';
import type { TableColumns } from '@/ipc/types';

/**
 * Cached table/column names per connection, feeding editor autocomplete.
 *
 * Fetched once per connection and reused across every tab. Autocomplete has to
 * answer within a keystroke, so it reads this synchronously rather than hitting
 * IPC — which also means a stale entry is possible after a DDL change, hence
 * the explicit `invalidate`.
 */
interface SchemaCacheState {
  /** connectionId → tables. Absent means "not fetched yet". */
  byConnection: Record<string, TableColumns[]>;
  loading: Set<string>;

  ensure: (connectionId: string) => Promise<void>;
  invalidate: (connectionId: string) => void;
  get: (connectionId: string | null) => TableColumns[];
}

export const useSchemaCache = create<SchemaCacheState>((set, get) => ({
  byConnection: {},
  loading: new Set(),

  ensure: async (connectionId) => {
    const { byConnection, loading } = get();
    if (byConnection[connectionId] || loading.has(connectionId)) return;

    set((s) => ({ loading: new Set(s.loading).add(connectionId) }));
    try {
      // `null` schema lets the driver pick its default (`public` on Postgres,
      // the single namespace on SQLite).
      const tables = await ipc.schemaSnapshot(connectionId, null);
      set((s) => ({ byConnection: { ...s.byConnection, [connectionId]: tables } }));
    } catch {
      // A failed snapshot costs autocomplete, nothing else. Staying silent
      // beats an error banner over a feature the user did not ask for.
    } finally {
      set((s) => {
        const next = new Set(s.loading);
        next.delete(connectionId);
        return { loading: next };
      });
    }
  },

  invalidate: (connectionId) =>
    set((s) => {
      const next = { ...s.byConnection };
      delete next[connectionId];
      return { byConnection: next };
    }),

  get: (connectionId) => (connectionId ? (get().byConnection[connectionId] ?? []) : []),
}));

/**
 * Shape the cache into what `@codemirror/lang-sql` expects for its `schema`
 * option: an object of table name → column names.
 *
 * Tables are registered under both their bare and schema-qualified names so
 * `users.` and `public.users.` both complete.
 */
export function toCompletionSchema(tables: TableColumns[]): Record<string, string[]> {
  const out: Record<string, string[]> = {};
  for (const t of tables) {
    out[t.name] = t.columns;
    if (t.schema) out[`${t.schema}.${t.name}`] = t.columns;
  }
  return out;
}
