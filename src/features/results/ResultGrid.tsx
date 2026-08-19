import { useVirtualizer } from '@tanstack/react-virtual';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { IconFilter, IconKey, IconSortAsc, IconSortDesc, IconTrash } from '@/components/icons';
import type { EditValue, FilterOp, ResultSet, Value } from '@/ipc/types';
import type { EditState } from '@/lib/edits';
import { isRowDeleted, stagedValue } from '@/lib/edits';
import type { GridFilter, SortState } from '@/lib/grid';
import { formatValue, isNumeric } from '@/lib/value';

/** Editing hooks the grid calls; absent when the result is read-only. */
export interface GridEditing {
  edits: EditState;
  onCellEdit: (rowIndex: number, column: string, value: EditValue) => void;
  onToggleDelete: (rowIndex: number) => void;
  onInsertCellEdit: (insertIndex: number, column: string, value: EditValue) => void;
  onRemoveInsert: (insertIndex: number) => void;
}

const ROW_HEIGHT = 26;
const HEADER_HEIGHT = 28;
const FILTER_HEIGHT = 26;
const ROW_NUM_WIDTH = 52;
const MIN_COL_WIDTH = 56;

/**
 * The result grid.
 *
 * Rows are virtualized so a full 1000-row page renders a couple of dozen DOM
 * nodes instead of thousands. Column widths are measured once from a sample of
 * rows rather than per-render, since re-measuring on scroll is what makes naive
 * grids feel sluggish.
 *
 * Sorting and filtering are *controlled* — this component reports intent and
 * renders what it is given. That is what lets a table tab push the same
 * gestures down into SQL while a query tab handles them in the client.
 */
export function ResultGrid({
  result,
  primaryKey = [],
  sort,
  onSortChange,
  filters,
  onFiltersChange,
  showFilters,
  onCellClick,
  editing,
}: {
  result: ResultSet;
  primaryKey?: string[];
  sort: SortState | null;
  onSortChange: (s: SortState | null) => void;
  filters: GridFilter[];
  onFiltersChange: (f: GridFilter[]) => void;
  showFilters: boolean;
  onCellClick?: (value: Value, column: string) => void;
  editing?: GridEditing;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [selected, setSelected] = useState<{ row: number; col: number } | null>(null);
  /** Which cell is in text-entry mode. `row: -1 - n` addresses staged insert n. */
  const [editingCell, setEditingCell] = useState<{ row: number; col: number } | null>(null);

  const columnKeys = useMemo(
    () => result.columns.map((c, i) => `${c.name}\u0000${i}`),
    [result.columns],
  );

  const [widths, setWidths] = useState<Record<string, number>>({});
  const [order, setOrder] = useState<number[]>([]);
  const [dragCol, setDragCol] = useState<number | null>(null);

  // Reset layout whenever the shape of the result changes. Keeping widths from
  // a previous query's columns would size the new ones arbitrarily.
  //
  // `editingCell` resets here too: it addresses a cell by row/column index
  // into *this* `result`, so a new one makes that index meaningless — most
  // refreshes already blur the open editor first, which commits or discards
  // it, but nothing guarantees that for every path that can hand this
  // component a new result (a future live-refresh, say), so this is the
  // actual invariant rather than a coincidence of today's call sites.
  useEffect(() => {
    setWidths(measureWidths(result));
    setOrder(result.columns.map((_, i) => i));
    setSelected(null);
    setEditingCell(null);
  }, [result]);

  const visible = order.length === result.columns.length ? order : result.columns.map((_, i) => i);

  const widthOf = useCallback(
    (i: number) => widths[columnKeys[i] ?? ''] ?? 140,
    [widths, columnKeys],
  );

  const totalWidth = useMemo(
    () => visible.reduce((sum, i) => sum + widthOf(i), ROW_NUM_WIDTH),
    [visible, widthOf],
  );

  const rowVirtualizer = useVirtualizer({
    count: result.rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
  });

  const startResize = useCallback(
    (colIndex: number, e: React.PointerEvent) => {
      e.preventDefault();
      e.stopPropagation();
      const key = columnKeys[colIndex] ?? '';
      const startX = e.clientX;
      const startW = widths[key] ?? 140;

      const onMove = (ev: PointerEvent) => {
        const next = Math.max(MIN_COL_WIDTH, startW + (ev.clientX - startX));
        setWidths((w) => ({ ...w, [key]: next }));
      };
      const onUp = () => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onUp);
      };
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUp);
    },
    [columnKeys, widths],
  );

  const setFilter = useCallback(
    (column: string, patch: Partial<GridFilter>) => {
      const existing = filters.find((f) => f.column === column);
      const merged: GridFilter = {
        column,
        op: patch.op ?? existing?.op ?? 'contains',
        value: patch.value ?? existing?.value ?? '',
      };
      const others = filters.filter((f) => f.column !== column);
      // Drop a filter once it has neither a value nor a null-check operator,
      // so an emptied box stops constraining the result.
      const keep = merged.value !== '' || merged.op === 'isNull' || merged.op === 'isNotNull';
      onFiltersChange(keep ? [...others, merged] : others);
    },
    [filters, onFiltersChange],
  );

  if (result.columns.length === 0) {
    return (
      <div
        className="flex h-full items-center justify-center text-[12px]"
        style={{ color: 'var(--text-faint)' }}
      >
        No columns returned
      </div>
    );
  }

  const pkSet = new Set(primaryKey);
  const headerBlock = HEADER_HEIGHT + (showFilters ? FILTER_HEIGHT : 0);

  /**
   * Move the selection with the arrow keys.
   *
   * The grid already tracked a selected cell and gave the scroll container
   * `tabIndex={0}`, but nothing moved that selection without a mouse, so the
   * whole results table was unreachable from the keyboard. Enter opens the
   * cell inspector, matching what a click does.
   */
  function onGridKeyDown(e: React.KeyboardEvent) {
    // Let the cell editor keep its own keys.
    if (editingCell) return;

    const rowCount = result.rows.length;
    const colCount = visible.length;
    if (rowCount === 0 || colCount === 0) return;

    const current = selected ?? { row: 0, col: 0 };
    const clamp = (n: number, max: number) => Math.max(0, Math.min(n, max));

    let next: { row: number; col: number } | null = null;
    switch (e.key) {
      case 'ArrowDown':
        next = { ...current, row: clamp(current.row + 1, rowCount - 1) };
        break;
      case 'ArrowUp':
        next = { ...current, row: clamp(current.row - 1, rowCount - 1) };
        break;
      case 'ArrowRight':
        next = { ...current, col: clamp(current.col + 1, colCount - 1) };
        break;
      case 'ArrowLeft':
        next = { ...current, col: clamp(current.col - 1, colCount - 1) };
        break;
      case 'Home':
        next = { ...current, col: 0 };
        break;
      case 'End':
        next = { ...current, col: colCount - 1 };
        break;
      case 'PageDown':
        next = { ...current, row: clamp(current.row + 20, rowCount - 1) };
        break;
      case 'PageUp':
        next = { ...current, row: clamp(current.row - 20, rowCount - 1) };
        break;
      case 'Enter': {
        const columnIndex = visible[current.col];
        const cell =
          columnIndex === undefined ? undefined : result.rows[current.row]?.[columnIndex];
        const column = columnIndex === undefined ? undefined : result.columns[columnIndex];
        if (cell && column && onCellClick) {
          e.preventDefault();
          onCellClick(cell, column.name);
        }
        return;
      }
      default:
        return;
    }

    e.preventDefault();
    setSelected(next);
    // Keep the newly selected row on screen; the rows are virtualized, so an
    // off-screen row has no element to scroll into view.
    rowVirtualizer.scrollToIndex(next.row, { align: 'auto' });
  }

  return (
    <div
      ref={parentRef}
      className="h-full overflow-auto"
      tabIndex={0}
      role="grid"
      aria-rowcount={result.rows.length}
      aria-colcount={result.columns.length}
      onKeyDown={onGridKeyDown}
    >
      <div style={{ width: totalWidth, minWidth: '100%' }}>
        {/* Header. Sticky so column names stay visible while scrolling. */}
        <div className="sticky top-0 z-10" style={{ background: 'var(--bg-inset)' }}>
          <div className="flex" style={{ height: HEADER_HEIGHT }}>
            <div
              className="shrink-0 border-r border-b"
              style={{ width: ROW_NUM_WIDTH, borderColor: 'var(--border)' }}
            />
            {visible.map((ci) => {
              const col = result.columns[ci]!;
              const sorted = sort?.column === col.name;
              return (
                <div
                  key={columnKeys[ci]}
                  className="group relative flex shrink-0 cursor-pointer select-none items-center gap-1 border-r border-b px-2"
                  style={{
                    width: widthOf(ci),
                    borderColor: 'var(--border)',
                    opacity: dragCol === ci ? 0.4 : 1,
                  }}
                  title={`${col.name} · ${col.typeName}`}
                  onClick={() => onSortChange(nextFor(sort, col.name))}
                  draggable
                  onDragStart={() => setDragCol(ci)}
                  onDragEnd={() => setDragCol(null)}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={(e) => {
                    e.preventDefault();
                    if (dragCol === null || dragCol === ci) return;
                    setOrder((cur) => moveItem(cur, dragCol, ci));
                    setDragCol(null);
                  }}
                >
                  {pkSet.has(col.name) && (
                    <IconKey size={10} className="shrink-0" style={{ color: 'var(--warning)' }} />
                  )}
                  <span className="truncate text-[11.5px] font-semibold">{col.name}</span>
                  <span
                    className="truncate text-[10px] font-normal"
                    style={{ color: 'var(--text-faint)' }}
                  >
                    {col.typeName}
                  </span>
                  <span className="flex-1" />
                  {sorted &&
                    (sort!.desc ? (
                      <IconSortDesc size={11} style={{ color: 'var(--accent)' }} />
                    ) : (
                      <IconSortAsc size={11} style={{ color: 'var(--accent)' }} />
                    ))}

                  {/* Resize grip. Wider than the visible line so it is
                      actually grabbable. */}
                  <span
                    className="absolute top-0 right-0 h-full w-2 translate-x-1 cursor-col-resize"
                    onClick={(e) => e.stopPropagation()}
                    onPointerDown={(e) => startResize(ci, e)}
                  />
                </div>
              );
            })}
          </div>

          {showFilters && (
            <div className="flex" style={{ height: FILTER_HEIGHT }}>
              <div
                className="flex shrink-0 items-center justify-center border-r border-b"
                style={{ width: ROW_NUM_WIDTH, borderColor: 'var(--border)' }}
              >
                <IconFilter size={11} style={{ color: 'var(--text-faint)' }} />
              </div>
              {visible.map((ci) => {
                const col = result.columns[ci]!;
                const f = filters.find((x) => x.column === col.name);
                return (
                  <div
                    key={columnKeys[ci]}
                    className="flex shrink-0 items-center gap-0.5 border-r border-b px-1"
                    style={{ width: widthOf(ci), borderColor: 'var(--border)' }}
                  >
                    <select
                      className="shrink-0 bg-transparent text-[10px] outline-none"
                      style={{ color: 'var(--text-faint)', width: 22 }}
                      value={f?.op ?? 'contains'}
                      onChange={(e) => setFilter(col.name, { op: e.target.value as FilterOp })}
                      title="Filter operator"
                    >
                      <option value="contains">⊃</option>
                      <option value="equals">=</option>
                      <option value="notEquals">≠</option>
                      <option value="startsWith">^</option>
                      <option value="greaterThan">&gt;</option>
                      <option value="lessThan">&lt;</option>
                      <option value="isNull">∅</option>
                      <option value="isNotNull">∃</option>
                    </select>
                    <input
                      className="min-w-0 flex-1 bg-transparent text-[11px] outline-none"
                      style={{ color: 'var(--text)' }}
                      value={f?.value ?? ''}
                      disabled={f?.op === 'isNull' || f?.op === 'isNotNull'}
                      placeholder="filter"
                      onChange={(e) => setFilter(col.name, { value: e.target.value })}
                    />
                  </div>
                );
              })}
            </div>
          )}
        </div>

        <div style={{ height: rowVirtualizer.getTotalSize(), position: 'relative' }}>
          {rowVirtualizer.getVirtualItems().map((v) => {
            const row = result.rows[v.index];
            if (!row) return null;
            const deleted = editing ? isRowDeleted(editing.edits, v.index) : false;
            return (
              <div
                key={v.key}
                className="absolute left-0 flex"
                style={{
                  top: 0,
                  transform: `translateY(${v.start}px)`,
                  height: ROW_HEIGHT,
                  width: totalWidth,
                  background: deleted
                    ? 'color-mix(in srgb, var(--danger) 14%, var(--bg))'
                    : v.index % 2 === 1
                      ? 'var(--row-alt)'
                      : 'var(--bg)',
                }}
              >
                <div
                  className="group/num flex shrink-0 items-center justify-end gap-1 border-r px-1.5 text-[11px] tabular-nums"
                  style={{
                    width: ROW_NUM_WIDTH,
                    borderColor: 'var(--border)',
                    color: 'var(--text-faint)',
                  }}
                >
                  {editing && (
                    <button
                      type="button"
                      className="opacity-0 transition-opacity group-hover/num:opacity-70 hover:!opacity-100"
                      title={deleted ? 'Keep this row' : 'Delete this row'}
                      onClick={() => editing.onToggleDelete(v.index)}
                      style={deleted ? { opacity: 1, color: 'var(--danger)' } : undefined}
                    >
                      <IconTrash size={10} />
                    </button>
                  )}
                  <span style={deleted ? { textDecoration: 'line-through' } : undefined}>
                    {v.index + 1}
                  </span>
                </div>
                {visible.map((ci) => {
                  const cell = row[ci];
                  const name = result.columns[ci]!.name;
                  const staged = editing ? stagedValue(editing.edits, v.index, name) : undefined;
                  return (
                    <Cell
                      key={columnKeys[ci]}
                      value={cell}
                      staged={staged}
                      width={widthOf(ci)}
                      selected={selected?.row === v.index && selected?.col === ci}
                      editing={editingCell?.row === v.index && editingCell?.col === ci}
                      editable={!!editing && !deleted}
                      onClick={() => {
                        setSelected({ row: v.index, col: ci });
                        if (cell) onCellClick?.(cell, name);
                      }}
                      onStartEdit={() => setEditingCell({ row: v.index, col: ci })}
                      onCommit={(value) => {
                        editing?.onCellEdit(v.index, name, value);
                        setEditingCell(null);
                      }}
                      onCancel={() => setEditingCell(null)}
                    />
                  );
                })}
              </div>
            );
          })}
        </div>

        {/* Staged inserts sit below the fetched page, outside the virtualizer
            since there are only ever a handful. */}
        {editing?.edits.inserts.map((row, insertIndex) => (
          <div
            key={`insert-${insertIndex}`}
            className="flex"
            style={{
              height: ROW_HEIGHT,
              width: totalWidth,
              background: 'color-mix(in srgb, var(--success) 12%, var(--bg))',
            }}
          >
            <div
              className="flex shrink-0 items-center justify-between border-r px-1.5"
              style={{ width: ROW_NUM_WIDTH, borderColor: 'var(--border)' }}
            >
              <span className="text-[11px]" style={{ color: 'var(--success)' }}>
                new
              </span>
              <button
                type="button"
                className="opacity-60 hover:opacity-100"
                title="Discard this new row"
                onClick={() => editing.onRemoveInsert(insertIndex)}
              >
                <IconTrash size={10} />
              </button>
            </div>
            {visible.map((ci) => {
              const name = result.columns[ci]!.name;
              return (
                <Cell
                  key={columnKeys[ci]}
                  value={undefined}
                  staged={row[name]}
                  width={widthOf(ci)}
                  selected={false}
                  editing={editingCell?.row === -1 - insertIndex && editingCell?.col === ci}
                  editable
                  onClick={() => setSelected(null)}
                  onStartEdit={() => setEditingCell({ row: -1 - insertIndex, col: ci })}
                  onCommit={(value) => {
                    editing.onInsertCellEdit(insertIndex, name, value);
                    setEditingCell(null);
                  }}
                  onCancel={() => setEditingCell(null)}
                />
              );
            })}
          </div>
        ))}
      </div>

      {result.rows.length === 0 && (
        <div
          className="flex items-center justify-center py-8 text-[12px]"
          style={{ color: 'var(--text-faint)', paddingTop: headerBlock ? 24 : 0 }}
        >
          No rows match
        </div>
      )}
    </div>
  );
}

/** Cycle ascending → descending → unsorted, mirroring `nextSort` in lib/grid. */
function nextFor(current: SortState | null, column: string): SortState | null {
  if (current?.column !== column) return { column, desc: false };
  if (!current.desc) return { column, desc: true };
  return null;
}

function moveItem(list: number[], from: number, to: number): number[] {
  const fromPos = list.indexOf(from);
  const toPos = list.indexOf(to);
  if (fromPos < 0 || toPos < 0) return list;
  const next = [...list];
  const [moved] = next.splice(fromPos, 1);
  next.splice(toPos, 0, moved!);
  return next;
}

function Cell({
  value,
  staged,
  width,
  selected,
  editing,
  editable,
  onClick,
  onStartEdit,
  onCommit,
  onCancel,
}: {
  value: Value | undefined;
  staged: EditValue | undefined;
  width: number;
  selected: boolean;
  editing: boolean;
  editable: boolean;
  onClick: () => void;
  onStartEdit: () => void;
  onCommit: (value: EditValue) => void;
  onCancel: () => void;
}) {
  // What is displayed: the staged edit if there is one, otherwise the stored
  // value. A staged cell is tinted so unsaved work is never mistaken for data
  // that is actually in the database.
  const dirty = staged !== undefined;
  const showsNull = dirty ? staged.kind === 'null' : !value || value.kind === 'null';
  const isDefault = dirty && staged.kind === 'default';

  const text = dirty
    ? staged.kind === 'text'
      ? staged.value
      : ''
    : value
      ? formatValue(value)
      : '';
  const numeric = !dirty && value ? isNumeric(value) : false;

  if (editing) {
    return <CellEditor width={width} initial={text} onCommit={onCommit} onCancel={onCancel} />;
  }

  const placeholder = isDefault ? 'default' : showsNull ? 'NULL' : text;

  return (
    <div
      className="selectable shrink-0 cursor-default truncate border-r px-2 leading-[26px]"
      style={{
        width,
        borderColor: 'var(--border)',
        textAlign: numeric ? 'right' : 'left',
        fontFamily: numeric || showsNull || isDefault ? 'var(--font-mono)' : undefined,
        // NULL is rendered as a dimmed italic marker so it cannot be confused
        // with an empty string or the literal text 'NULL'.
        color: showsNull || isDefault ? 'var(--null)' : undefined,
        fontStyle: showsNull || isDefault ? 'italic' : undefined,
        boxShadow: selected ? 'inset 0 0 0 2px var(--accent)' : undefined,
        background: dirty
          ? 'color-mix(in srgb, var(--warning) 22%, transparent)'
          : selected
            ? 'var(--accent-soft)'
            : undefined,
      }}
      onClick={onClick}
      onDoubleClick={() => editable && onStartEdit()}
      title={
        editable
          ? `${showsNull ? 'NULL' : text}\n\nDouble-click to edit`
          : showsNull
            ? 'NULL'
            : text
      }
    >
      {placeholder}
    </div>
  );
}

/**
 * Inline text entry for one cell.
 *
 * Enter commits, Escape abandons, and ⌘⌫ writes a real NULL — the grid has no
 * other way to express "no value" as distinct from the empty string, and
 * guessing from an emptied box would write the wrong one half the time.
 */
function CellEditor({
  width,
  initial,
  onCommit,
  onCancel,
}: {
  width: number;
  initial: string;
  onCommit: (value: EditValue) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState(initial);

  return (
    <input
      // This input is mounted only because the user just chose to edit this
      // cell, so focusing it is completing their action, not stealing focus.
      // biome-ignore lint/a11y/noAutofocus: see above
      autoFocus
      className="shrink-0 border-r px-2 text-[12px] leading-[26px] outline-none"
      style={{
        width,
        height: ROW_HEIGHT,
        borderColor: 'var(--accent)',
        background: 'var(--bg)',
        color: 'var(--text)',
        boxShadow: 'inset 0 0 0 2px var(--accent)',
        fontFamily: 'var(--font-mono)',
      }}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => onCommit({ kind: 'text', value: draft })}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          onCommit({ kind: 'text', value: draft });
        } else if (e.key === 'Escape') {
          e.preventDefault();
          onCancel();
        } else if ((e.metaKey || e.ctrlKey) && e.key === 'Backspace') {
          e.preventDefault();
          onCommit({ kind: 'null' });
        }
      }}
      title="Enter to save · Esc to cancel · ⌘⌫ for NULL"
    />
  );
}

/**
 * Pick a width per column from the header plus a sample of rows.
 *
 * Sampling the first 100 rows keeps this cheap while still sizing sensibly;
 * measuring every row of a large result would cost more than rendering it.
 */
function measureWidths(result: ResultSet): Record<string, number> {
  const CHAR = 7.1;
  const MAX = 420;
  const sample = result.rows.slice(0, 100);
  const out: Record<string, number> = {};

  result.columns.forEach((col, i) => {
    let longest = col.name.length + col.typeName.length + 3;
    for (const row of sample) {
      const cell = row[i];
      if (!cell) continue;
      const len = cell.kind === 'null' ? 4 : formatValue(cell).length;
      if (len > longest) longest = len;
    }
    out[`${col.name}\u0000${i}`] = Math.min(
      MAX,
      Math.max(MIN_COL_WIDTH + 16, Math.round(longest * CHAR) + 28),
    );
  });

  return out;
}
