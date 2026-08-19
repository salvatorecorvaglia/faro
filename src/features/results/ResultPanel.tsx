import { useEffect, useMemo, useRef, useState } from 'react';

import { IconBraces, IconClose, IconFilter, IconGrid } from '@/components/icons';
import { EmptyState, ErrorBanner } from '@/components/ui';
import type { ResultSet, StatementResult, Value } from '@/ipc/types';
import { applyGridOps, type GridFilter, type SortState } from '@/lib/grid';
import { formatDuration, formatRowCount, formatValue, resultToJson, toJson } from '@/lib/value';
import { type GridEditing, ResultGrid } from './ResultGrid';

type ViewMode = 'grid' | 'json';

/** Matches the table-browsing debounce (TableTab), so both feel alike. */
const FILTER_DEBOUNCE_MS = 200;

/**
 * Sorting and filtering work one of two ways, chosen by the caller — and the
 * two are mutually exclusive, not just independently optional: passing
 * `clientSideSort` together with a controlled `sort`/`onSortChange` used to
 * type-check while silently ignoring the controlled props, because both were
 * simply optional. A discriminated union makes that combination impossible to
 * construct instead of merely wrong at runtime.
 */
type SortFilterMode =
  | {
      /** The rows are already in hand (an ad-hoc query result), so sorting
       * and filtering run locally over that page. */
      clientSideSort: true;
    }
  | {
      clientSideSort?: false;
      /** Pushes the same gestures down into SQL — a table tab, where sorting
       * a fetched page of a ten-million-row table would order the wrong
       * thousand rows and quietly mislead. */
      sort: SortState | null;
      onSortChange: (s: SortState | null) => void;
      filters: GridFilter[];
      onFiltersChange: (f: GridFilter[]) => void;
    };

type ResultPanelProps = {
  statements: StatementResult[];
  browseResult: ResultSet | null;
  primaryKey?: string[];
  error: string | null;
  activeIndex: number;
  onActiveIndexChange: (i: number) => void;
  running: boolean;
  editing?: GridEditing;
} & SortFilterMode;

/** The results pane below the editor. */
export function ResultPanel(props: ResultPanelProps) {
  const {
    statements,
    browseResult,
    primaryKey,
    error,
    activeIndex,
    onActiveIndexChange,
    running,
    editing,
  } = props;

  const [mode, setMode] = useState<ViewMode>('grid');
  const [inspect, setInspect] = useState<{ value: Value; column: string } | null>(null);
  const [showFilters, setShowFilters] = useState(false);

  // Local sort/filter state, used only in client-side mode.
  const [localSort, setLocalSort] = useState<SortState | null>(null);
  const [localFilters, setLocalFilters] = useState<GridFilter[]>([]);

  // Filter text arrives a character at a time. Re-filtering and re-sorting
  // the full result set synchronously on every keystroke is exactly the cost
  // table browsing debounces server-side (TableTab) — this is the same idea
  // applied locally: the input stays responsive because `localFilters`
  // (what the grid displays) updates at once, while the expensive
  // `applyGridOps` pass runs against a debounced copy.
  const [debouncedFilters, setDebouncedFilters] = useState<GridFilter[]>([]);
  const filterTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => clearTimeout(filterTimer.current ?? undefined), []);

  const sort = props.clientSideSort ? localSort : props.sort;
  const filters = props.clientSideSort ? localFilters : props.filters;
  const setSort = props.clientSideSort ? setLocalSort : props.onSortChange;
  const setFilters = props.clientSideSort
    ? (next: GridFilter[]) => {
        setLocalFilters(next);
        clearTimeout(filterTimer.current ?? undefined);
        filterTimer.current = setTimeout(() => setDebouncedFilters(next), FILTER_DEBOUNCE_MS);
      }
    : props.onFiltersChange;

  // A new query's columns may be unrelated to the last one's, so carrying a
  // sort or filter across would silently drop rows the user did not exclude.
  const clientSideSort = props.clientSideSort ?? false;
  useEffect(() => {
    if (!clientSideSort) return;
    setLocalSort(null);
    setLocalFilters([]);
    setDebouncedFilters([]);
  }, [statements, clientSideSort]);

  // A table tab has a single result set; a query tab has one per statement.
  //
  // Memoized on `browseResult` so the wrapper keeps a stable identity. Built
  // inline it was a new object on every render, which flowed into ResultGrid's
  // `[result]` reset effect and threw away the user's column widths, column
  // ordering and cell selection — and re-measured every column — whenever
  // anything at all re-rendered the tab.
  const active: StatementResult | null = useMemo(
    () =>
      browseResult
        ? { sql: '', outcome: { type: 'rows' as const, ...browseResult }, error: null }
        : null,
    [browseResult],
  );

  const shown = active ?? statements[activeIndex] ?? null;
  const rawRows = shown?.outcome?.type === 'rows' ? shown.outcome : null;

  // In client-side mode the grid shows a locally sorted/filtered view. In
  // pushdown mode the server already did the work, so this passes through.
  const rows = useMemo(() => {
    if (!rawRows) return null;
    return clientSideSort ? applyGridOps(rawRows, sort, debouncedFilters) : rawRows;
  }, [rawRows, clientSideSort, sort, debouncedFilters]);

  if (error) {
    return (
      <div className="h-full overflow-auto p-3">
        <ErrorBanner message={error} />
      </div>
    );
  }

  if (running && !shown) {
    return <EmptyState title="Running…" />;
  }

  if (!shown) {
    return <EmptyState title="No results yet" hint="Press ⌘↵ to run the query." />;
  }

  const hidden = rawRows && rows ? rawRows.rows.length - rows.rows.length : 0;

  return (
    <div className="flex h-full flex-col">
      <div
        className="flex h-8 shrink-0 items-center gap-1 border-b px-2"
        style={{ borderColor: 'var(--border)', background: 'var(--bg-subtle)' }}
      >
        {statements.length > 1 && (
          <div className="flex items-center gap-0.5 pr-2">
            {statements.map((s, i) => (
              <button
                type="button"
                key={i}
                onClick={() => onActiveIndexChange(i)}
                className="btn px-1.5 py-0.5 text-[11px]"
                style={
                  i === activeIndex
                    ? { background: 'var(--accent-soft)', color: 'var(--accent)' }
                    : { color: 'var(--text-muted)' }
                }
                title={s.sql}
              >
                {i + 1}
                {s.error && <span style={{ color: 'var(--danger)' }}>!</span>}
              </button>
            ))}
          </div>
        )}

        <StatusLine result={shown} hidden={hidden} />

        <div className="flex-1" />

        {rawRows && (
          <div className="flex items-center gap-0.5">
            <button
              type="button"
              className="btn btn-ghost px-1.5"
              onClick={() => setShowFilters((v) => !v)}
              style={showFilters || filters.length > 0 ? { color: 'var(--accent)' } : undefined}
              title="Column filters"
            >
              <IconFilter size={12} />
              {filters.length > 0 && (
                <span className="text-[10px] tabular-nums">{filters.length}</span>
              )}
            </button>
            <button
              type="button"
              className="btn btn-ghost px-1.5"
              onClick={() => setMode('grid')}
              style={mode === 'grid' ? { color: 'var(--accent)' } : undefined}
              title="Grid view"
            >
              <IconGrid size={13} />
            </button>
            <button
              type="button"
              className="btn btn-ghost px-1.5"
              onClick={() => setMode('json')}
              style={mode === 'json' ? { color: 'var(--accent)' } : undefined}
              title="JSON view"
            >
              <IconBraces size={13} />
            </button>
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1">
        {shown.error ? (
          <div className="h-full overflow-auto p-3">
            <ErrorBanner message={shown.error} />
          </div>
        ) : !rows ? (
          <EmptyState
            title={`${formatRowCount(
              shown.outcome?.type === 'affected' ? shown.outcome.rowsAffected : 0,
            )} rows affected`}
            hint="The statement completed successfully."
          />
        ) : mode === 'grid' ? (
          <ResultGrid
            result={rows}
            primaryKey={primaryKey ?? []}
            sort={sort}
            onSortChange={setSort}
            filters={filters}
            onFiltersChange={setFilters}
            showFilters={showFilters}
            onCellClick={(value, column) => setInspect({ value, column })}
            editing={editing}
          />
        ) : (
          <JsonView result={rows} />
        )}
      </div>

      {inspect && <CellInspector {...inspect} onClose={() => setInspect(null)} />}
    </div>
  );
}

function StatusLine({ result, hidden }: { result: StatementResult; hidden: number }) {
  if (result.error) {
    return (
      <span className="text-[11px] font-medium" style={{ color: 'var(--danger)' }}>
        Failed
      </span>
    );
  }
  const o = result.outcome;
  if (!o) return null;

  if (o.type === 'rows') {
    return (
      <span
        className="flex items-center gap-1.5 text-[11px]"
        style={{ color: 'var(--text-muted)' }}
      >
        <span className="tabular-nums">{formatRowCount(o.rows.length)} rows</span>
        <span style={{ color: 'var(--text-faint)' }}>·</span>
        <span className="tabular-nums">{formatDuration(o.elapsedMs)}</span>
        {hidden > 0 && (
          <>
            <span style={{ color: 'var(--text-faint)' }}>·</span>
            <span style={{ color: 'var(--accent)' }}>
              {formatRowCount(hidden)} hidden by filters
            </span>
          </>
        )}
        {o.truncated && (
          <>
            <span style={{ color: 'var(--text-faint)' }}>·</span>
            {/* Being explicit matters: a silently capped result would let
                someone draw a conclusion from partial data. */}
            <span style={{ color: 'var(--warning)' }}>truncated — more rows exist</span>
          </>
        )}
      </span>
    );
  }

  return (
    <span className="flex items-center gap-1.5 text-[11px]" style={{ color: 'var(--text-muted)' }}>
      <span className="tabular-nums">{formatRowCount(o.rowsAffected)} rows affected</span>
      <span style={{ color: 'var(--text-faint)' }}>·</span>
      <span className="tabular-nums">{formatDuration(o.elapsedMs)}</span>
    </span>
  );
}

function JsonView({ result }: { result: ResultSet }) {
  // Cap what gets stringified: pretty-printing 1000 wide rows can take long
  // enough to drop frames, and nobody reads past the first screenful anyway.
  const LIMIT = 200;
  const text = useMemo(() => {
    const capped: ResultSet = { ...result, rows: result.rows.slice(0, LIMIT) };
    return JSON.stringify(resultToJson(capped), null, 2);
  }, [result]);

  return (
    <div className="h-full overflow-auto">
      <pre className="selectable p-3 font-mono text-[12px] leading-relaxed">{text}</pre>
      {result.rows.length > LIMIT && (
        <p className="px-3 pb-3 text-[11px]" style={{ color: 'var(--text-faint)' }}>
          Showing the first {LIMIT} of {formatRowCount(result.rows.length)} rows.
        </p>
      )}
    </div>
  );
}

/** Full-value viewer, for cells the grid had to truncate. */
function CellInspector({
  value,
  column,
  onClose,
}: {
  value: Value;
  column: string;
  onClose: () => void;
}) {
  const pretty =
    value.kind === 'json'
      ? JSON.stringify(toJson(value), null, 2)
      : value.kind === 'null'
        ? 'NULL'
        : formatValue(value);

  return (
    <div
      className="flex max-h-56 shrink-0 flex-col border-t"
      style={{ borderColor: 'var(--border)', background: 'var(--bg-subtle)' }}
    >
      <div className="flex h-7 shrink-0 items-center gap-2 px-2.5">
        <span className="text-[11px] font-semibold">{column}</span>
        <span className="text-[10.5px]" style={{ color: 'var(--text-faint)' }}>
          {value.kind}
        </span>
        <div className="flex-1" />
        <button
          type="button"
          className="btn btn-ghost px-1"
          onClick={onClose}
          aria-label="Close inspector"
        >
          <IconClose size={12} />
        </button>
      </div>
      <pre className="selectable min-h-0 flex-1 overflow-auto px-2.5 pb-2.5 font-mono text-[12px]">
        {pretty}
      </pre>
    </div>
  );
}
