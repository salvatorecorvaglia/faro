import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { IconDownload, IconPlus, IconRefresh, IconUpload } from '@/components/icons';
import { Spinner } from '@/components/ui';
import { ApplyBar, ReadOnlyNotice } from '@/features/results/ApplyBar';
import { ResultPanel } from '@/features/results/ResultPanel';
import { ExportDialog } from '@/features/transfer/ExportDialog';
import { ImportDialog } from '@/features/transfer/ImportDialog';
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
} from '@/lib/edits';
import type { GridFilter, SortState } from '@/lib/grid';
import { useConnections } from '@/state/connections';
import { type Tab, useTabs } from '@/state/tabs';

let browseCounter = 0;
const PAGE_SIZE = 1000;

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
): string {
  // Checked first: it is the user's own choice, so it explains the state better
  // than anything about the table would, and it is the one they can undo.
  if (connectionReadOnly) {
    return 'This connection is open read-only. Turn off “Open read-only” in the connection settings to make changes.';
  }
  if (kind !== 'table') return 'Views are read-only.';
  if (engine === 'clickhouse') {
    return 'ClickHouse tables are read-only here: a MergeTree primary key sorts rows rather than uniquely identifying them, so an edit could not be limited to one row.';
  }
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
  const engine = (conn?.engine as Engine | undefined) ?? null;
  /**
   * The backend refuses writes on a read-only connection regardless, but a grid
   * that lets you stage twenty edits and only objects at Apply is a worse way to
   * find out. The button states should tell the truth up front.
   */
  const connectionReadOnly = conn?.readOnly ?? false;
  const [detail, setDetail] = useState<TableDetail | null>(null);
  const [offset, setOffset] = useState(0);
  const [sort, setSort] = useState<SortState | null>(null);
  const [filters, setFilters] = useState<GridFilter[]>([]);

  const [edits, setEdits] = useState<EditState>(emptyEdits);
  const [applying, setApplying] = useState(false);
  const [applyError, setApplyError] = useState<string | null>(null);
  const [noticeDismissed, setNoticeDismissed] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);

  const tabRef = useRef(tab);
  tabRef.current = tab;

  const editable =
    !connectionReadOnly && (detail?.primaryKey.length ? detail.kind === 'table' : false);
  const dirty = hasChanges(edits);

  // Mirror the dirty flag into the store. Only the store is around when the
  // tab is switched away from or closed — this component is unmounted by then,
  // taking `edits` with it — so it is what has to know there is work to lose.
  useEffect(() => {
    update(tab.id, { dirty });
  }, [dirty, tab.id, update]);

  const load = useCallback(
    async (nextOffset: number, nextSort: SortState | null, nextFilters: GridFilter[]) => {
      const t = tabRef.current;
      if (!t.connectionId || !t.table) return;

      const queryId = `b-${++browseCounter}`;
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
        update(t.id, { running: false, queryId: null, browseResult: result });
        setOffset(nextOffset);
        // Staged edits address rows by their position in the page, so any
        // reload invalidates them. Discarding is the honest outcome — silently
        // reapplying them to different rows would be far worse.
        setEdits(emptyEdits());
      } catch (e) {
        update(t.id, { running: false, queryId: null, error: ipc.errorMessage(e) });
      }
    },
    [update],
  );

  useEffect(() => {
    if (!tab.connectionId || !tab.table) return;
    ipc
      .describeTable(tab.connectionId, tab.table)
      .then(setDetail)
      .catch(() => setDetail(null));
    load(0, null, []);
    // Loads once per table; sort, filter and paging re-run it explicitly.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.connectionId, tab.table?.schema, tab.table?.name]);

  /** Confirm before throwing away unsaved work. */
  const guardUnsaved = useCallback(
    (action: () => void) => {
      if (dirty && !confirm('Discard your unsaved changes?')) return;
      action();
    },
    [dirty],
  );

  // Changing the sort or a filter changes which rows belong on page one, so
  // both reset the offset rather than leaving the user deep in a stale page.
  const onSortChange = useCallback(
    (next: SortState | null) =>
      guardUnsaved(() => {
        setSort(next);
        load(0, next, filters);
      }),
    [guardUnsaved, load, filters],
  );

  // Filter text arrives a character at a time. Applying it immediately meant
  // one `browse_table` round trip per keystroke — and, when the grid was dirty,
  // a blocking `confirm()` per keystroke too. The input stays responsive
  // because the filter state updates at once; only the query is deferred.
  //
  // The same 200ms the history search uses, so the two feel alike.
  const filterTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => clearTimeout(filterTimer.current ?? undefined), []);

  const onFiltersChange = useCallback(
    (next: GridFilter[]) => {
      setFilters(next);
      clearTimeout(filterTimer.current ?? undefined);
      filterTimer.current = setTimeout(() => {
        guardUnsaved(() => load(0, sort, next));
      }, FILTER_DEBOUNCE_MS);
    },
    [guardUnsaved, load, sort],
  );

  const pendingChanges = useMemo(() => {
    if (!tab.browseResult || !detail) return [];
    return toPendingChanges(tab.browseResult, detail.primaryKey, edits);
  }, [tab.browseResult, detail, edits]);

  async function onApply() {
    if (!tab.connectionId || !tab.table) return;
    setApplying(true);
    setApplyError(null);
    try {
      await ipc.applyChanges(tab.connectionId, tab.table, pendingChanges);
      setEdits(emptyEdits());
      // Re-read so the grid shows what the database actually stored, including
      // defaults and triggers that changed values on the way in.
      await load(offset, sort, filters);
    } catch (e) {
      setApplyError(ipc.errorMessage(e));
    } finally {
      setApplying(false);
    }
  }

  const gridEditing = editable
    ? {
        edits,
        onCellEdit: (rowIndex: number, column: string, value: EditValue) => {
          const columnIndex = tab.browseResult?.columns.findIndex((c) => c.name === column) ?? -1;
          const original =
            columnIndex >= 0 ? tab.browseResult?.rows[rowIndex]?.[columnIndex] : undefined;
          setEdits((e) => setCell(e, rowIndex, column, value, original));
        },
        onToggleDelete: (rowIndex: number) => setEdits((e) => toggleDelete(e, rowIndex)),
        onInsertCellEdit: (i: number, column: string, value: EditValue) =>
          setEdits((e) => setInsertCell(e, i, column, value)),
        onRemoveInsert: (i: number) => setEdits((e) => removeInsert(e, i)),
      }
    : undefined;

  const hasMore = tab.browseResult?.truncated ?? false;

  return (
    <div className="flex h-full flex-col">
      <div
        className="flex h-9 shrink-0 items-center gap-2 border-b px-2"
        style={{ borderColor: 'var(--border)' }}
      >
        <span className="text-[12px] font-semibold">
          {tab.table?.schema ? `${tab.table.schema}.` : ''}
          {tab.table?.name}
        </span>
        {detail && (
          <span className="text-[11px]" style={{ color: 'var(--text-faint)' }}>
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
          <span className="text-[11px] tabular-nums" style={{ color: 'var(--text-muted)' }}>
            {offset + 1}–{offset + (tab.browseResult?.rows.length ?? 0)}
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

      {!dirty && detail && !editable && !noticeDismissed && (
        <ReadOnlyNotice
          reason={readOnlyReason(detail.kind, engine, connectionReadOnly)}
          onDismiss={() => setNoticeDismissed(true)}
        />
      )}

      <ExportDialog
        open={exportOpen}
        onClose={() => setExportOpen(false)}
        result={tab.browseResult}
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
