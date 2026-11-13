import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { IconDownload, IconPlus, IconRefresh, IconUpload } from '@/components/icons';
import { Spinner } from '@/components/ui';
import { ApplyBar, ReadOnlyNotice } from '@/features/results/ApplyBar';
import { ResultPanel } from '@/features/results/ResultPanel';
import { ExportDialog } from '@/features/transfer/ExportDialog';
import { ImportDialog } from '@/features/transfer/ImportDialog';
import { useAsyncAction } from '@/hooks/useAsyncAction';
import * as ipc from '@/ipc';
import type { EditValue, Engine, TableDetail } from '@/ipc/types';
import {
  addRow,
  changeCount,
  type EditState,
  emptyEdits,
  hasChanges,
  removeInsert,
  setCell,
  setInsertCell,
  toggleDelete,
  toPendingChanges,
  unaddressableKeyReason,
} from '@/lib/edits';
import type { GridFilter, SortState } from '@/lib/grid';
import { confirmDialog } from '@/state/confirm';
import { useConnections } from '@/state/connections';
import { type Tab, useTabs } from '@/state/tabs';
import { PAGE_SIZE, useTableBrowse } from './useTableBrowse';

/** Matches the history search debounce, so filtering feels the same. */
const FILTER_DEBOUNCE_MS = 200;

/**
 * Why a table cannot be edited.
 *
 * ClickHouse gets its own wording: it *does* have a primary key, so saying it
 * has none would be wrong. The key is a sparse sorting index rather than a
 * unique constraint, which is the actual reason rows cannot be addressed.
 */
function readOnlyReason(
  kind: TableDetail['kind'],
  engine: Engine | null,
  connectionReadOnly: boolean,
  keyBlocker: string | null,
): string {
  // Checked first: it is the user's own choice, so it explains the state better
  // than anything about the table would, and it is the one they can undo.
  if (connectionReadOnly) {
    return 'This connection is open read-only. Turn off “Open read-only” in the connection settings and reconnect to make changes.';
  }
  if (kind !== 'table') return 'Views are read-only.';
  if (engine === 'clickhouse') {
    return 'ClickHouse tables are read-only here: a MergeTree primary key sorts rows rather than uniquely identifying them, so an edit could not be limited to one row.';
  }
  if (keyBlocker) return keyBlocker;
  return 'This table has no primary key, so rows cannot be edited safely — an update could not be limited to one row.';
}

/**
 * Browsing and editing one table.
 *
 * Sorting, filtering and paging all happen server-side through `browse_table`.
 * That is the whole point: sorting the fetched page of a ten-million-row table
 * would order the wrong thousand rows, and the user would have no way to tell.
 *
 * Edits are staged locally and written only on Apply, as one transaction.
 * A table with no primary key stays read-only, because there is no way to
 * address exactly one of its rows.
 */
export function TableTab({ tab }: { tab: Tab }) {
  const update = useTabs((s) => s.update);
  const conn = useConnections((s) => s.items.find((c) => c.id === tab.connectionId));
  const engine = ((conn?.activeConfig ?? conn)?.engine as Engine | undefined) ?? null;
  /**
   * The backend refuses writes on a read-only connection regardless, but a grid
   * that lets you stage twenty edits and only objects at Apply is a worse way to
   * find out. The button states should tell the truth up front.
   */
  const connectionReadOnly = (conn?.activeConfig ?? conn)?.readOnly ?? false;
  const [edits, setEdits] = useState<EditState>(emptyEdits);
  const {
    busy: applying,
    error: applyError,
    setError: setApplyError,
    run: apply,
  } = useAsyncAction();
  const [exportOpen, setExportOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);

  const { detail, offset, sort, setSort, filters, setFilters, appliedFilters, load } =
    useTableBrowse(tab, () => setEdits(emptyEdits()));

  // A key the grid cannot rebuild exactly (binary, float, …) would never find
  // its row again, so such a table is read-only until it can.
  const keyBlocker = useMemo(
    () => unaddressableKeyReason(tab.browseResult, detail?.primaryKey ?? []),
    [tab.browseResult, detail],
  );
  const editable =
    !connectionReadOnly &&
    !keyBlocker &&
    (detail?.primaryKey.length ? detail.kind === 'table' : false);
  const dirty = hasChanges(edits);

  // Mirror the dirty flag into the store. Only the store is around when the
  // tab is switched away from or closed — this component is unmounted by then,
  // taking `edits` with it — so it is what has to know there is work to lose.
  useEffect(() => {
    update(tab.id, { dirty });
  }, [dirty, tab.id, update]);

  /**
   * Confirm before throwing away unsaved work.
   *
   * Reports whether it went ahead, so a caller that already moved some UI state
   * optimistically can put it back when the user declines.
   */
  const guardUnsaved = useCallback(
    async (action: () => void): Promise<boolean> => {
      if (dirty && !(await confirmDialog('Discard your unsaved changes?', { danger: true }))) {
        return false;
      }
      action();
      return true;
    },
    [dirty],
  );

  const filterTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Changing the sort or a filter changes which rows belong on page one, so
  // both reset the offset rather than leaving the user deep in a stale page.
  const onSortChange = useCallback(
    (next: SortState | null) => {
      void guardUnsaved(() => {
        clearTimeout(filterTimer.current ?? undefined);
        setSort(next);
        void load(0, next, filters);
      });
    },
    [guardUnsaved, load, filters, setSort],
  );

  // Filter text arrives a character at a time. Applying it immediately meant
  // one `browse_table` round trip per keystroke — and, when the grid was dirty,
  // a blocking `confirm()` per keystroke too. The input stays responsive
  // because the filter state updates at once; only the query is deferred.
  //
  // The same 200ms the history search uses, so the two feel alike.
  useEffect(() => () => clearTimeout(filterTimer.current ?? undefined), []);

  const onFiltersChange = useCallback(
    (next: GridFilter[]) => {
      setFilters(next);
      clearTimeout(filterTimer.current ?? undefined);
      filterTimer.current = setTimeout(() => {
        void guardUnsaved(() => load(0, sort, next)).then((applied) => {
          // Declining keeps the staged edits, which means the query still
          // reflects the previous filters. Without this the filter row went on
          // displaying a filter that was never run — the grid and the boxes
          // above it disagreeing about what the user is looking at.
          if (!applied) setFilters(appliedFilters.current);
        });
      }, FILTER_DEBOUNCE_MS);
    },
    [guardUnsaved, load, sort, setFilters, appliedFilters],
  );

  const pendingChanges = useMemo(() => {
    if (!tab.browseResult || !detail) return [];
    return toPendingChanges(tab.browseResult, detail.primaryKey, edits);
  }, [tab.browseResult, detail, edits]);

  async function onApply() {
    if (!tab.connectionId || !tab.table) return;
    await apply(async () => {
      await ipc.applyChanges(tab.connectionId!, tab.table!, pendingChanges);
      setEdits(emptyEdits());
      // Re-read defaults and trigger changes after the transaction commits.
      await load(offset, sort, filters);
    });
  }

  // Memoized: a fresh object literal on every render would change identity each
  // time a single cell is edited, re-rendering the whole grid underneath.
  const browseResult = tab.browseResult;
  const gridEditing = useMemo(
    () =>
      editable
        ? {
            edits,
            onCellEdit: (rowIndex: number, column: string, value: EditValue) => {
              const columnIndex = browseResult?.columns.findIndex((c) => c.name === column) ?? -1;
              const original =
                columnIndex >= 0 ? browseResult?.rows[rowIndex]?.[columnIndex] : undefined;
              setEdits((e) => setCell(e, rowIndex, column, value, original));
            },
            onToggleDelete: (rowIndex: number) => setEdits((e) => toggleDelete(e, rowIndex)),
            onInsertCellEdit: (i: number, column: string, value: EditValue) =>
              setEdits((e) => setInsertCell(e, i, column, value)),
            onRemoveInsert: (i: number) => setEdits((e) => removeInsert(e, i)),
          }
        : undefined,
    [editable, edits, browseResult],
  );

  const hasMore = tab.browseResult?.truncated ?? false;
  const rowsOnPage = tab.browseResult?.rows.length ?? 0;

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b px-2 border-line">
        <span className="text-sm font-semibold">
          {tab.table?.schema ? `${tab.table.schema}.` : ''}
          {tab.table?.name}
        </span>
        {detail && (
          <span className="text-xs text-faint">
            {detail.columns.length} columns
            {detail.primaryKey.length > 0 && ` · PK: ${detail.primaryKey.join(', ')}`}
          </span>
        )}

        <button
          type="button"
          className="btn btn-ghost px-1.5"
          onClick={() => guardUnsaved(() => load(offset, sort, filters))}
          disabled={tab.running}
          title="Refresh"
        >
          {tab.running ? <Spinner size={12} /> : <IconRefresh size={12} />}
        </button>

        {editable && detail && (
          <button
            type="button"
            className="btn btn-ghost"
            onClick={() => setEdits((e) => addRow(e, detail.columns))}
            title="Add a row"
          >
            <IconPlus size={12} /> Row
          </button>
        )}

        <button
          type="button"
          className="btn btn-ghost"
          onClick={() => setExportOpen(true)}
          disabled={!tab.browseResult}
          title="Export this table"
        >
          <IconDownload size={12} /> Export
        </button>

        {editable && (
          <button
            type="button"
            className="btn btn-ghost"
            onClick={() => setImportOpen(true)}
            title="Import a file into this table"
          >
            <IconUpload size={12} /> Import
          </button>
        )}

        <div className="flex-1" />

        <div className="flex items-center gap-1.5">
          <button
            type="button"
            className="btn btn-outline py-1"
            onClick={() => guardUnsaved(() => load(Math.max(0, offset - PAGE_SIZE), sort, filters))}
            disabled={offset === 0 || tab.running}
          >
            Previous
          </button>
          <span className="text-xs tabular-nums text-muted">
            {/* Without a zero case this read "1–0" on an empty table — a range
                running backwards, describing rows that do not exist. */}
            {rowsOnPage === 0 ? 'No rows' : `Rows ${offset + 1}–${offset + rowsOnPage}`}
          </span>
          <button
            type="button"
            className="btn btn-outline py-1"
            onClick={() => guardUnsaved(() => load(offset + PAGE_SIZE, sort, filters))}
            disabled={!hasMore || tab.running}
          >
            Next
          </button>
        </div>
      </div>

      <div className="min-h-0 flex-1">
        <ResultPanel
          statements={[]}
          browseResult={tab.browseResult}
          primaryKey={detail?.primaryKey ?? []}
          error={tab.error}
          activeIndex={0}
          onActiveIndexChange={() => {}}
          running={tab.running}
          sort={sort}
          onSortChange={onSortChange}
          filters={filters}
          onFiltersChange={onFiltersChange}
          editing={gridEditing}
        />
      </div>

      {dirty && (
        <ApplyBar
          changeCount={changeCount(edits)}
          applying={applying}
          error={applyError}
          onApply={onApply}
          onDiscard={() => {
            setEdits(emptyEdits());
            setApplyError(null);
          }}
          onPreview={() => ipc.previewChanges(tab.connectionId!, tab.table!, pendingChanges)}
          onDismissError={() => setApplyError(null)}
        />
      )}

      {!dirty && detail && !editable && !tab.readOnlyNoticeDismissed && (
        <ReadOnlyNotice
          reason={readOnlyReason(detail.kind, engine, connectionReadOnly, keyBlocker)}
          onDismiss={() => update(tab.id, { readOnlyNoticeDismissed: true })}
        />
      )}

      <ExportDialog
        open={exportOpen}
        onClose={() => setExportOpen(false)}
        result={tab.browseResult}
        source={tab.resultQueryId ? { queryId: tab.resultQueryId, statement: 0 } : null}
        connectionId={tab.connectionId}
        table={tab.table}
        defaultName={tab.table?.name ?? 'export'}
      />

      <ImportDialog
        open={importOpen}
        onClose={() => setImportOpen(false)}
        connectionId={tab.connectionId}
        table={tab.table}
        columns={detail?.columns ?? []}
        onImported={() => load(offset, sort, filters)}
      />
    </div>
  );
}
