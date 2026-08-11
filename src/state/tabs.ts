import { create } from 'zustand';

import type { ResultSet, StatementResult, TableRef } from '@/ipc/types';

export type TabKind = 'query' | 'table';

/**
 * One unit of work. Two tabs can run against different connections at the same
 * time.
 *
 * Everything a tab must survive being switched away from lives here, because
 * `App` mounts only the active tab and the others are unmounted. State held in
 * the tab components themselves — a table tab's page offset, sort and filters,
 * a result panel's view mode — is deliberately *not* preserved and resets on
 * return. Staged edits used to be in that category too, which lost data
 * silently; `dirty` is what lets the store prompt before that happens.
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
  /**
   * Table tabs: whether the grid holds staged INSERT/UPDATE/DELETE that have
   * not been applied.
   *
   * Lives here rather than only inside `TableTab` because only the store knows
   * a tab is about to be switched away from or closed. The component's own
   * copy dies with it — `App` mounts just the active tab — so without this the
   * edits vanished silently.
   */
  dirty: boolean;
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
    dirty: false,
  };
}

/**
 * Ask before abandoning a tab holding unapplied edits.
 *
 * Returns true when it is safe to proceed. Kept out of the store actions so
 * both `setActive` and `closeTab` phrase it identically.
 */
function confirmLeaving(tab: Tab | undefined): boolean {
  if (!tab?.dirty) return true;
  return confirm(`"${tab.title}" has unsaved changes that have not been applied.\n\nDiscard them?`);
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

  closeTab: (id) => {
    // Closing a tab with staged edits throws them away for good.
    if (!confirmLeaving(get().tabs.find((t) => t.id === id))) return;
    set((s) => {
      const index = s.tabs.findIndex((t) => t.id === id);
      const tabs = s.tabs.filter((t) => t.id !== id);
      if (s.activeId !== id) return { tabs, activeId: s.activeId };
      // Focus the neighbour, preferring the one to the left, as editors do.
      const neighbour = tabs[index - 1] ?? tabs[index] ?? tabs[tabs.length - 1];
      return { tabs, activeId: neighbour?.id ?? null };
    });
  },

  // Switching away unmounts the tab, which discards its staged edits. Every
  // other route out of a dirty grid — refresh, sort, filter, paging — already
  // confirms; this one silently destroyed the work.
  setActive: (id) => {
    const { activeId, tabs } = get();
    if (id === activeId) return;
    if (!confirmLeaving(tabs.find((t) => t.id === activeId))) return;
    set({ activeId: id });
  },

  update: (id, patch) =>
    set((s) => ({
      tabs: s.tabs.map((t) => (t.id === id ? { ...t, ...patch } : t)),
    })),

  activeTab: () => {
    const { tabs, activeId } = get();
    return tabs.find((t) => t.id === activeId);
  },
}));
