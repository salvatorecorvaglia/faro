import { useVirtualizer } from '@tanstack/react-virtual';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { IconFilter, IconKey, IconSortAsc, IconSortDesc, IconTrash } from '@/components/icons';
import type { EditValue, FilterOp, ResultSet, Value } from '@/ipc/types';
import type { EditState } from '@/lib/edits';
import { isRowDeleted, stagedValue } from '@/lib/edits';
import { type GridFilter, nextSort, type SortState } from '@/lib/grid';
import { Cell } from './GridCell';
import {
  FILTER_HEIGHT,
  HEADER_HEIGHT,
  MIN_COL_WIDTH,
  measureWidths,
  ROW_HEIGHT,
  ROW_NUM_WIDTH,
} from './gridLayout';

/** Editing hooks the grid calls; absent when the result is read-only. */
export interface GridEditing {
  edits: EditState;
  onCellEdit: (rowIndex: number, column: string, value: EditValue) => void;
  onToggleDelete: (rowIndex: number) => void;
  onInsertCellEdit: (insertIndex: number, column: string, value: EditValue) => void;
  onRemoveInsert: (insertIndex: number) => void;
}

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
  // Keyed on `result.columns`, not `result`. In client-side mode `applyGridOps`
  // returns `{ ...result, rows }` — a fresh object — on every sort and every
  // debounced filter keystroke, so depending on `result` threw away the user's
  // column widths, their column ordering and the selected cell each time they
  // sorted or typed in a filter box. `applyGridOps` passes the same `columns`
  // array straight through, so this identity changes only when the result
  // genuinely has a new shape, which is what the reset is actually for.
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
    // `result` is read inside but deliberately not a dependency; see above.
  }, [result.columns]);

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

  // Holds the active drag's teardown so it can be run from the unmount effect
  // below too, not just from `pointerup` — otherwise a resize that is still in
  // progress when the grid unmounts (switching tabs, the result reloading)
  // leaves these listeners on `window` for the rest of the app's life.
  const activeResizeCleanup = useRef<(() => void) | null>(null);

  useEffect(() => {
    return () => activeResizeCleanup.current?.();
  }, []);

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
        activeResizeCleanup.current = null;
      };
      window.addEventListener('pointermove', onMove);
      window.addEventListener('pointerup', onUp);
      activeResizeCleanup.current = onUp;
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
                  role="columnheader"
                  // Sorting was mouse-only: the header was a plain div with a
                  // click handler, so nothing announced the current sort and
                  // there was no way to reach it from the keyboard.
                  aria-sort={sorted ? (sort!.desc ? 'descending' : 'ascending') : 'none'}
                  tabIndex={0}
                  className="group relative flex shrink-0 cursor-pointer select-none items-center gap-1 border-r border-b px-2"
                  style={{
                    width: widthOf(ci),
                    borderColor: 'var(--border)',
                    opacity: dragCol === ci ? 0.4 : 1,
                  }}
                  title={`${col.name} · ${col.typeName}`}
                  onClick={() => onSortChange(nextSort(sort, col.name))}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter' || e.key === ' ') {
                      e.preventDefault();
                      onSortChange(nextSort(sort, col.name));
                    }
                  }}
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
                      // Named per column: a screen reader otherwise hears the
                      // same "Filter operator" on every one of them.
                      aria-label={`Filter operator for ${col.name}`}
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
                      aria-label={`Filter ${col.name}`}
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
                role="row"
                aria-rowindex={v.index + 1}
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

function moveItem(list: number[], from: number, to: number): number[] {
  const fromPos = list.indexOf(from);
  const toPos = list.indexOf(to);
  if (fromPos < 0 || toPos < 0) return list;
  const next = [...list];
  const [moved] = next.splice(fromPos, 1);
  next.splice(toPos, 0, moved!);
  return next;
}
