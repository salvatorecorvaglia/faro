import { create } from 'zustand';

import type { ResultSet, StatementResult, TableRef } from '@/ipc/types';

export type TabKind = 'query' | 'table';

/**
 * One unit of work. Each tab owns its editor text and its results, so switching
 * tabs never loses state and two tabs can run against different connections at
 * the same time.
 */
export interface Tab {
  id: string;
  kind: TabKind;
  title: string;
  connectionId: string | null;

  /** Query tabs: editor contents. */
  sql: string;
  /** Table tabs: which table is being browsed. */
  table: TableRef | null;

  /** Results of the last run. A script yields one entry per statement. */
  results: StatementResult[];
  /** Table tabs hold a single result set instead. */
  browseResult: ResultSet | null;

  running: boolean;
  /** Identifies the in-flight query so it can be cancelled. */
  queryId: string | null;
  error: string | null;
  /** Which statement's results are showing, when a script produced several. */
  activeResultIndex: number;
}

interface TabState {
  tabs: Tab[];
  activeId: string | null;

  openQueryTab: (connectionId: string | null, sql?: string, title?: string) => string;
  openTableTab: (connectionId: string, table: TableRef) => string;
  closeTab: (id: string) => void;
  setActive: (id: string) => void;
  update: (id: string, patch: Partial<Tab>) => void;
  activeTab: () => Tab | undefined;
}

let counter = 0;
const nextId = () => `tab-${++counter}`;

function baseTab(kind: TabKind, connectionId: string | null): Tab {
  return {
    id: nextId(),
    kind,
    title: '',
    connectionId,
    sql: '',
    table: null,
    results: [],
    browseResult: null,
    running: false,
    queryId: null,
    error: null,
    activeResultIndex: 0,
  };
}

export const useTabs = create<TabState>((set, get) => ({
  tabs: [],
  activeId: null,

  openQueryTab: (connectionId, sql = '', title) => {
    const tab: Tab = {
      ...baseTab('query', connectionId),
      title: title ?? `Query ${get().tabs.filter((t) => t.kind === 'query').length + 1}`,
      sql,
    };
    set((s) => ({ tabs: [...s.tabs, tab], activeId: tab.id }));
    return tab.id;
  },

  openTableTab: (connectionId, table) => {
    // Reuse an existing tab for the same table rather than stacking duplicates
    // every time the user clicks it in the tree.
    const existing = get().tabs.find(
      (t) =>
        t.kind === 'table' &&
        t.connectionId === connectionId &&
        t.table?.name === table.name &&
        t.table?.schema === table.schema,
    );
    if (existing) {
      set({ activeId: existing.id });
      return existing.id;
    }

    const tab: Tab = {
      ...baseTab('table', connectionId),
      title: table.name,
      table,
    };
    set((s) => ({ tabs: [...s.tabs, tab], activeId: tab.id }));
    return tab.id;
  },

  closeTab: (id) =>
    set((s) => {
      const index = s.tabs.findIndex((t) => t.id === id);
      const tabs = s.tabs.filter((t) => t.id !== id);
      if (s.activeId !== id) return { tabs, activeId: s.activeId };
      // Focus the neighbour, preferring the one to the left, as editors do.
      const neighbour = tabs[index - 1] ?? tabs[index] ?? tabs[tabs.length - 1];
      return { tabs, activeId: neighbour?.id ?? null };
    }),

  setActive: (id) => set({ activeId: id }),

  update: (id, patch) =>
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === id ? { ...t, ...patch } : t)),
    })),

  activeTab: () => {
    const { tabs, activeId } = get();
    return tabs.find((t) => t.id === activeId);
  },
}));
