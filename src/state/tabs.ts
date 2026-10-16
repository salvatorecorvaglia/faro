import { create } from 'zustand';
import { createJSONStorage, persist, type StateStorage } from 'zustand/middleware';

import type { ResultSet, StatementResult, TableRef } from '@/ipc/types';
import type { GridFilter, SortState } from '@/lib/grid';
import { confirmDialog } from './confirm';

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
  /**
   * Query tabs: most rows a run may return.
   *
   * Lives on the tab rather than in the component so it survives being
   * switched away from, like everything else here — and so two tabs can hold
   * different limits. The backend clamps whatever arrives to `MAX_PAGE`.
   */
  rowLimit: number;
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
  /**
   * Table tabs: whether the "this table is read-only" notice has been
   * dismissed.
   *
   * On the tab rather than in `TableTab` for the same reason as `dirty`: `App`
   * mounts only the active tab, so component state here died on every tab
   * switch and the notice came back each time the user returned — having
   * already been told, and having dismissed it.
   */
  readOnlyNoticeDismissed: boolean;
  /**
   * Table tabs: the page, sort and filters `browseResult` was loaded with.
   *
   * Here rather than in `TableTab` for the same reason as `dirty`: only the
   * active tab is mounted, so as component state they reset on every switch —
   * returning to a table meant page one, unsorted and unfiltered, re-queried
   * from scratch, wherever the user had left it.
   */
  /**
   * The query id the shown results (a run's, or a page's) came back under.
   *
   * Exports name the result by this id and the backend writes its own copy,
   * rather than the UI sending rows back — large binary values reach the UI
   * only as their size, so the UI's copy is not a faithful one.
   */
  resultQueryId: string | null;
  browseOffset: number;
  browseSort: SortState | null;
  browseFilters: GridFilter[];
}

interface TabState {
  tabs: Tab[];
  activeId: string | null;

  openQueryTab: (connectionId: string | null, sql?: string, title?: string) => string;
  openTableTab: (connectionId: string, table: TableRef) => string;
  closeTab: (id: string) => Promise<void>;
  setActive: (id: string) => Promise<void>;
  update: (id: string, patch: Partial<Tab>) => void;
  activeTab: () => Tab | undefined;
}

let counter = 0;
/**
 * A tab id unique across launches, not only within one.
 *
 * Query tabs are restored after a restart (see `persist` below), and a plain
 * counter starts again at 1 — so the first tab opened after relaunching would
 * have reused a restored tab's id, and the two would have shared one entry.
 */
const launch = Date.now().toString(36);
const nextId = () => `tab-${launch}-${++counter}`;

/** Where query tabs are kept between launches. */
export const TABS_STORAGE_KEY = 'faro.tabs';

/**
 * `localStorage`, written at most every `WRITE_DELAY_MS`.
 *
 * Every keystroke in the editor updates the store, and `localStorage` writes
 * are synchronous — serializing every tab on each key would put that cost on
 * the typing path. The latest value is kept and written shortly after, and
 * flushed when the page is hidden or closed so a quick quit does not lose it.
 *
 * Every access is guarded: storage can be unavailable or full, and a failure
 * here must never break the app — it only means tabs are not restored.
 */
const WRITE_DELAY_MS = 250;
const pendingWrites = new Map<string, string>();
let writeTimer: ReturnType<typeof setTimeout> | null = null;

/** Write anything still waiting. Exported so tests need not wait out the delay. */
export function flushTabStorage(): void {
  if (writeTimer) clearTimeout(writeTimer);
  writeTimer = null;
  for (const [key, value] of pendingWrites) {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Storage full or unavailable: tabs simply are not restored next time.
    }
  }
  pendingWrites.clear();
}

if (typeof window !== 'undefined') {
  window.addEventListener('pagehide', flushTabStorage);
  window.addEventListener('beforeunload', flushTabStorage);
}

const debouncedStorage: StateStorage = {
  getItem: (key) => {
    try {
      return pendingWrites.get(key) ?? localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  setItem: (key, value) => {
    pendingWrites.set(key, value);
    if (!writeTimer) writeTimer = setTimeout(flushTabStorage, WRITE_DELAY_MS);
  },
  removeItem: (key) => {
    pendingWrites.delete(key);
    try {
      localStorage.removeItem(key);
    } catch {
      // Nothing to clean up if storage is unavailable.
    }
  },
};

/** The part of a query tab that is worth restoring. */
type PersistedTab = Pick<Tab, 'id' | 'title' | 'connectionId' | 'sql' | 'rowLimit'>;

interface PersistedTabs {
  tabs: PersistedTab[];
  activeId: string | null;
}

/**
 * Rows a query returns unless the user asks for more.
 *
 * Mirrors `DEFAULT_PAGE` in the Rust driver layer. The backend is the authority
 * and clamps anything larger than its `MAX_PAGE`.
 */
export const DEFAULT_ROW_LIMIT = 1_000;

/** The choices offered in the query toolbar. The largest is the backend cap. */
export const ROW_LIMIT_CHOICES = [1_000, 10_000, 50_000, 100_000] as const;

function baseTab(kind: TabKind, connectionId: string | null): Tab {
  return {
    id: nextId(),
    kind,
    title: '',
    connectionId,
    sql: '',
    rowLimit: DEFAULT_ROW_LIMIT,
    table: null,
    results: [],
    browseResult: null,
    running: false,
    queryId: null,
    error: null,
    activeResultIndex: 0,
    dirty: false,
    readOnlyNoticeDismissed: false,
    resultQueryId: null,
    browseOffset: 0,
    browseSort: null,
    browseFilters: [],
  };
}

/**
 * Ask before abandoning a tab holding unapplied edits.
 *
 * Resolves true when it is safe to proceed. Kept out of the store actions so
 * both `setActive` and `closeTab` phrase it identically.
 */
function confirmLeaving(tab: Tab | undefined): Promise<boolean> {
  if (!tab?.dirty) return Promise.resolve(true);
  return confirmDialog(
    `"${tab.title}" has unsaved changes that have not been applied.\n\nDiscard them?`,
    { confirmLabel: 'Discard', danger: true },
  );
}

/**
 * Move focus to `id`, asking first when the outgoing tab holds staged edits.
 *
 * `App` mounts only the active tab, so *any* focus change unmounts the previous
 * one and takes its `edits` with it. `setActive` and `closeTab` already guarded
 * that; the two openers set `activeId` directly and slipped past it, so opening
 * a table from the sidebar or pressing ⌘T discarded staged edits silently.
 *
 * Focus moves synchronously when there is nothing to lose, which is almost
 * always — only a genuinely dirty outgoing tab defers it behind the prompt.
 * That keeps the openers' synchronous contract intact for every ordinary open.
 *
 * Declining leaves the newly opened tab in the background rather than throwing
 * away the click that created it; no work is lost either way.
 */
function focusAfterGuard(
  set: (partial: Partial<TabState>) => void,
  get: () => TabState,
  id: string,
): void {
  const { activeId, tabs } = get();
  if (id === activeId) return;

  const outgoing = tabs.find((t) => t.id === activeId);
  if (!outgoing?.dirty) {
    set({ activeId: id });
    return;
  }

  void confirmLeaving(outgoing).then((ok) => {
    if (ok) set({ activeId: id });
  });
}

/**
 * Query tabs survive a restart.
 *
 * They lived only in memory, so quitting — or a crash, or an update relaunch —
 * threw away every unsaved query with no warning. Only what the user wrote is
 * kept: title, connection, SQL and row limit. Results are not, being large and
 * stale by definition, and table tabs are not either, since reopening one from
 * the tree is a click and its staged edits could not be applied safely to rows
 * re-read later anyway.
 */
const persistence = {
  name: TABS_STORAGE_KEY,
  version: 1,
  storage: createJSONStorage<PersistedTabs>(() => debouncedStorage),
  partialize: (s: TabState): PersistedTabs => {
    const tabs = s.tabs
      .filter((t) => t.kind === 'query')
      .map(({ id, title, connectionId, sql, rowLimit }) => ({
        id,
        title,
        connectionId,
        sql,
        rowLimit,
      }));
    return {
      tabs,
      activeId: tabs.some((t) => t.id === s.activeId) ? s.activeId : null,
    };
  },
  merge: (persisted: unknown, current: TabState): TabState => {
    const saved = persisted as Partial<PersistedTabs> | undefined;
    if (!saved?.tabs?.length) return current;
    // Rebuilt from the defaults, so nothing that only made sense in the last
    // session — "running", an in-flight query id, old results — comes back.
    const restored: Tab[] = saved.tabs.map((t) => ({
      ...baseTab('query', t.connectionId ?? null),
      id: t.id,
      title: t.title,
      sql: t.sql ?? '',
      rowLimit: t.rowLimit ?? DEFAULT_ROW_LIMIT,
    }));
    return {
      ...current,
      tabs: [...restored, ...current.tabs],
      activeId: current.activeId ?? saved.activeId ?? restored[0]?.id ?? null,
    };
  },
};

export const useTabs = create<TabState>()(
  persist<TabState, [], [], PersistedTabs>(
    (set, get) => ({
      tabs: [],
      activeId: null,

      openQueryTab: (connectionId, sql = '', title) => {
        const tab: Tab = {
          ...baseTab('query', connectionId),
          title: title ?? `Query ${get().tabs.filter((t) => t.kind === 'query').length + 1}`,
          sql,
        };
        set((s) => ({ tabs: [...s.tabs, tab] }));
        focusAfterGuard(set, get, tab.id);
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
          focusAfterGuard(set, get, existing.id);
          return existing.id;
        }

        const tab: Tab = {
          ...baseTab('table', connectionId),
          title: table.name,
          table,
        };
        set((s) => ({ tabs: [...s.tabs, tab] }));
        focusAfterGuard(set, get, tab.id);
        return tab.id;
      },

      closeTab: async (id) => {
        // Closing a tab with staged edits throws them away for good.
        if (!(await confirmLeaving(get().tabs.find((t) => t.id === id)))) return;
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
      setActive: async (id) => {
        const { activeId, tabs } = get();
        if (id === activeId) return;
        if (!(await confirmLeaving(tabs.find((t) => t.id === activeId)))) return;
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
    }),
    persistence,
  ),
);
