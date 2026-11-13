import { useCallback, useEffect, useRef, useState } from 'react';
import * as ipc from '@/ipc';
import type { TableDetail } from '@/ipc/types';
import type { GridFilter, SortState } from '@/lib/grid';
import { type Tab, useTabs } from '@/state/tabs';

let browseCounter = 0;
export const PAGE_SIZE = 1000;

/** Owns table metadata and page requests, discarding replies superseded by a newer load. */
export function useTableBrowse(tab: Tab, onPageLoaded: () => void) {
  const update = useTabs((s) => s.update);
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const onLoaded = useRef(onPageLoaded);
  onLoaded.current = onPageLoaded;
  const [detail, setDetail] = useState<TableDetail | null>(null);
  const [offset, setOffset] = useState(tab.browseOffset);
  const [sort, setSort] = useState<SortState | null>(tab.browseSort);
  const [filters, setFilters] = useState<GridFilter[]>(tab.browseFilters);
  const appliedFilters = useRef<GridFilter[]>(tab.browseFilters);
  const load = useCallback(
    async (nextOffset: number, nextSort: SortState | null, nextFilters: GridFilter[]) => {
      const t = useTabs.getState().tabs.find((candidate) => candidate.id === tabRef.current.id);
      if (!t?.connectionId || !t.table) return;

      const queryId = `b-${++browseCounter}`;
      // A load still in flight is about to be superseded. Its reply would be
      // ignored anyway (see below), but the query itself kept running on the
      // connection's single session and every later page queued behind it —
      // so a quick run of sorts or filters on a large table waited for each
      // abandoned one in turn. Cancelling frees the session for this one.
      if (t.queryId) {
        void ipc.cancelQuery(t.connectionId, t.queryId).catch(() => {});
      }
      update(t.id, { running: true, queryId, error: null });
      try {
        const result = await ipc.browseTable(
          t.connectionId,
          t.table,
          {
            limit: PAGE_SIZE,
            offset: nextOffset,
            sortColumn: nextSort?.column ?? null,
            sortDesc: nextSort?.desc ?? false,
            filters: nextFilters,
          },
          queryId,
        );
        // A newer load can start (and overwrite `queryId`) while this one is
        // still in flight — a sort click right after a filter edit, say.
        // Applying whichever response lands last regardless of order would
        // silently show the wrong page under a newer sort/filter/offset, so
        // only the most recently started request's result is ever committed.
        if (useTabs.getState().tabs.find((tab) => tab.id === t.id)?.queryId !== queryId) return;
        // What the page was loaded with goes into the store beside it, so a
        // return to this tab shows it as it was left.
        update(t.id, {
          running: false,
          queryId: null,
          browseResult: result,
          resultQueryId: queryId,
          browseOffset: nextOffset,
          browseSort: nextSort,
          browseFilters: nextFilters,
        });
        setOffset(nextOffset);
        // Remember what the displayed page was actually loaded with, so a
        // filter edit the user later declines to apply can be rolled back to it.
        appliedFilters.current = nextFilters;
        // Staged edits address rows by their position in the page, so any
        // reload invalidates them. Discarding is the honest outcome — silently
        // reapplying them to different rows would be far worse.
        onLoaded.current();
      } catch (e) {
        if (useTabs.getState().tabs.find((tab) => tab.id === t.id)?.queryId !== queryId) return;
        update(t.id, { running: false, queryId: null, error: ipc.errorMessage(e) });
      }
    },
    [update],
  );

  /**
   * Generation counter for `describeTable`.
   *
   * `load` already refuses a reply a newer request has superseded; the describe
   * running beside it had no such guard, so two quick table switches could
   * leave `detail` belonging to the *previous* table — and `detail.primaryKey`
   * decides both whether the grid is editable and what `toPendingChanges`
   * anchors its WHERE clauses on.
   */
  const describeSeq = useRef(0);

  // Runs once per table, keyed on its identity rather than on the `tab` object
  // or on `load`. Sort, filter and paging each re-run `load` explicitly, so
  // adding them here would reload the page twice for every one of those.
  // biome-ignore lint/correctness/useExhaustiveDependencies: once per table; see above
  useEffect(() => {
    if (!tab.connectionId || !tab.table) return;

    // Start from what this tab's page was last loaded with — its own, never
    // another table's: a tab is bound to one table, and the boxes must always
    // describe the data under them.
    const saved = tabRef.current;
    setSort(saved.browseSort);
    setFilters(saved.browseFilters);
    appliedFilters.current = saved.browseFilters;
    setOffset(saved.browseOffset);
    setDetail(null);

    const seq = ++describeSeq.current;
    ipc
      .describeTable(tab.connectionId, tab.table)
      .then((d) => {
        if (describeSeq.current === seq) setDetail(d);
      })
      .catch(() => {
        if (describeSeq.current === seq) setDetail(null);
      });

    // A page already in the store is shown as it was left rather than
    // re-queried on every switch back; Refresh re-reads it. `load` records
    // its own failures on the tab.
    if (!saved.browseResult) {
      void load(saved.browseOffset, saved.browseSort, saved.browseFilters);
    }
    return () => {
      describeSeq.current++;
    };
  }, [tab.connectionId, tab.table?.schema, tab.table?.name]);

  return { detail, offset, sort, setSort, filters, setFilters, appliedFilters, load };
}
