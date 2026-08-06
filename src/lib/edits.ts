import type { ColumnDetail, EditValue, PendingChange, ResultSet, Value } from '@/ipc/types';
import { formatValue } from './value';

/**
 * Staged grid edits, before the user commits them.
 *
 * Held as plain data keyed by row index within the currently displayed page,
 * so nothing is written until Apply and Discard is a matter of dropping the
 * object.
 */
export interface EditState {
  /** rowIndex → column → new value. */
  updates: Record<number, Record<string, EditValue>>;
  /** Row indices marked for deletion. */
  deletes: number[];
  /** Rows added at the bottom, not yet inserted. */
  inserts: Record<string, EditValue>[];
}

export const emptyEdits = (): EditState => ({ updates: {}, deletes: [], inserts: [] });

export function hasChanges(edits: EditState): boolean {
  return (
    edits.deletes.length > 0 ||
    edits.inserts.length > 0 ||
    Object.values(edits.updates).some((row) => Object.keys(row).length > 0)
  );
}

export function changeCount(edits: EditState): number {
  const updated = Object.values(edits.updates).filter((r) => Object.keys(r).length > 0).length;
  return updated + edits.deletes.length + edits.inserts.length;
}

/** The staged value for a cell, or undefined when it is untouched. */
export function stagedValue(
  edits: EditState,
  rowIndex: number,
  column: string,
): EditValue | undefined {
  return edits.updates[rowIndex]?.[column];
}

export function isRowDeleted(edits: EditState, rowIndex: number): boolean {
  return edits.deletes.includes(rowIndex);
}

/** Stage one cell edit, or clear it when the value returns to the original. */
export function setCell(
  edits: EditState,
  rowIndex: number,
  column: string,
  value: EditValue,
  original: Value | undefined,
): EditState {
  const row = { ...(edits.updates[rowIndex] ?? {}) };

  if (matchesOriginal(value, original)) {
    // Typing a value and then typing the original back should leave no trace,
    // so Apply does not run a statement that changes nothing.
    delete row[column];
  } else {
    row[column] = value;
  }

  const updates = { ...edits.updates };
  if (Object.keys(row).length === 0) delete updates[rowIndex];
  else updates[rowIndex] = row;

  return { ...edits, updates };
}

/** Whether a staged value is indistinguishable from what is already stored. */
function matchesOriginal(value: EditValue, original: Value | undefined): boolean {
  if (!original) return false;
  if (value.kind === 'null') return original.kind === 'null';
  if (value.kind === 'default') return false;
  // Compare against the rendered form, which is what the user was editing.
  // NULL renders as empty, so an original NULL never matches typed text.
  if (original.kind === 'null') return false;
  return value.value === formatValue(original);
}

export function toggleDelete(edits: EditState, rowIndex: number): EditState {
  const deletes = edits.deletes.includes(rowIndex)
    ? edits.deletes.filter((i) => i !== rowIndex)
    : [...edits.deletes, rowIndex];
  return { ...edits, deletes };
}

/**
 * Add a blank row.
 *
 * Every column starts as `default` so the database supplies its own values —
 * which is what lets an auto-increment key work without the user inventing one.
 */
export function addRow(edits: EditState, columns: ColumnDetail[]): EditState {
  const row: Record<string, EditValue> = {};
  for (const c of columns) row[c.name] = { kind: 'default' };
  return { ...edits, inserts: [...edits.inserts, row] };
}

export function setInsertCell(
  edits: EditState,
  insertIndex: number,
  column: string,
  value: EditValue,
): EditState {
  const inserts = edits.inserts.map((row, i) =>
    i === insertIndex ? { ...row, [column]: value } : row,
  );
  return { ...edits, inserts };
}

export function removeInsert(edits: EditState, insertIndex: number): EditState {
  return { ...edits, inserts: edits.inserts.filter((_, i) => i !== insertIndex) };
}

/**
 * Turn staged edits into the changes the backend applies.
 *
 * The key carries each primary key column's *original* value, taken from the
 * result set rather than from anything the user typed — so editing a primary
 * key still finds the row it started from.
 *
 * A row that is both edited and deleted yields only the delete: the update
 * would be undone a moment later anyway, and running it first risks tripping a
 * constraint for no reason.
 */
export function toPendingChanges(
  result: ResultSet,
  primaryKey: string[],
  edits: EditState,
): PendingChange[] {
  const changes: PendingChange[] = [];
  const columnIndex = new Map(result.columns.map((c, i) => [c.name, i]));

  const keyFor = (rowIndex: number) =>
    primaryKey.map((name) => {
      const i = columnIndex.get(name);
      const cell = i === undefined ? undefined : result.rows[rowIndex]?.[i];
      return { column: name, value: toEditValue(cell) };
    });

  for (const rowIndex of edits.deletes) {
    changes.push({ type: 'delete', key: keyFor(rowIndex) });
  }

  for (const [key, cells] of Object.entries(edits.updates)) {
    const rowIndex = Number(key);
    if (edits.deletes.includes(rowIndex)) continue;

    const entries = Object.entries(cells);
    if (entries.length === 0) continue;

    changes.push({
      type: 'update',
      key: keyFor(rowIndex),
      cells: entries.map(([column, value]) => ({ column, value })),
    });
  }

  for (const row of edits.inserts) {
    changes.push({
      type: 'insert',
      cells: Object.entries(row).map(([column, value]) => ({ column, value })),
    });
  }

  return changes;
}

/** Represent an existing cell as the edit value that would reproduce it. */
function toEditValue(cell: Value | undefined): EditValue {
  if (!cell || cell.kind === 'null') return { kind: 'null' };
  return { kind: 'text', value: formatValue(cell) };
}
