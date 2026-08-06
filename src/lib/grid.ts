import type { FilterOp, ResultSet, Value } from '@/ipc/types';
import { formatValue } from './value';

export interface SortState {
  column: string;
  desc: boolean;
}

export interface GridFilter {
  column: string;
  op: FilterOp;
  value: string;
}

/**
 * Order two values.
 *
 * Comparison is by type, not by rendered text: `10` must sort after `9`, which
 * string comparison gets wrong. NULL always sorts last regardless of direction,
 * the convention in most SQL clients — burying the rows you can see under the
 * ones you cannot is rarely what anyone wants.
 */
export function compareValues(a: Value | undefined, b: Value | undefined): number {
  const aNull = !a || a.kind === 'null';
  const bNull = !b || b.kind === 'null';
  if (aNull && bNull) return 0;
  if (aNull) return 1;
  if (bNull) return -1;

  const an = numericOf(a!);
  const bn = numericOf(b!);
  if (an !== null && bn !== null) return an < bn ? -1 : an > bn ? 1 : 0;

  if (a!.kind === 'bool' && b!.kind === 'bool') {
    return Number(a!.value) - Number(b!.value);
  }

  // localeCompare so accented text sorts next to its base letter rather than
  // after every ASCII character.
  return formatValue(a!).localeCompare(formatValue(b!), undefined, {
    numeric: true,
    sensitivity: 'base',
  });
}

/** Numeric view of a value, or null when it is not a number. */
function numericOf(v: Value): number | null {
  switch (v.kind) {
    case 'int':
    case 'float':
      return v.value;
    case 'decimal': {
      // Decimals arrive as strings to protect precision. Comparing them as
      // floats is fine for ordering even where it would be wrong for display.
      const n = Number(v.value);
      return Number.isFinite(n) ? n : null;
    }
    default:
      return null;
  }
}

/** Whether one cell satisfies one filter. */
export function matchesFilter(cell: Value | undefined, op: FilterOp, needle: string): boolean {
  const isNull = !cell || cell.kind === 'null';
  if (op === 'isNull') return isNull;
  if (op === 'isNotNull') return !isNull;
  if (isNull) return false;

  const text = formatValue(cell!);
  const lower = text.toLowerCase();
  const target = needle.toLowerCase();

  switch (op) {
    case 'equals':
      return lower === target;
    case 'notEquals':
      return lower !== target;
    case 'contains':
      return lower.includes(target);
    case 'startsWith':
      return lower.startsWith(target);
    case 'greaterThan':
    case 'lessThan': {
      const a = numericOf(cell!);
      const b = Number(needle);
      // Fall back to text ordering when either side is not numeric, so
      // `> 'm'` still does something sensible on a text column.
      const cmp =
        a !== null && Number.isFinite(b)
          ? a - b
          : text.localeCompare(needle, undefined, { numeric: true });
      return op === 'greaterThan' ? cmp > 0 : cmp < 0;
    }
    default:
      return true;
  }
}

/**
 * Apply filters then sorting to a result set, in the client.
 *
 * Used for ad-hoc query results, where the rows are already in hand. Table
 * browsing pushes the same operations down into SQL instead, because sorting
 * the fetched page of a ten-million-row table would show the wrong rows.
 *
 * Returns the input unchanged when there is nothing to do, so the grid's
 * memoization can skip re-rendering.
 */
export function applyGridOps(
  result: ResultSet,
  sort: SortState | null,
  filters: GridFilter[],
): ResultSet {
  const active = filters.filter((f) => f.value !== '' || f.op === 'isNull' || f.op === 'isNotNull');
  if (!sort && active.length === 0) return result;

  const indexOf = new Map(result.columns.map((c, i) => [c.name, i]));

  let rows = result.rows;

  if (active.length > 0) {
    rows = rows.filter((row) =>
      active.every((f) => {
        const i = indexOf.get(f.column);
        // A filter on a column that is no longer present is ignored rather
        // than filtering everything away.
        return i === undefined || matchesFilter(row[i], f.op, f.value);
      }),
    );
  }

  if (sort) {
    const i = indexOf.get(sort.column);
    if (i !== undefined) {
      // Copy before sorting: the unsorted rows belong to the tab's state and
      // sorting in place would mutate it.
      rows = [...rows].sort((ra, rb) => {
        const a = ra[i];
        const b = rb[i];
        // NULLs are ranked outside the direction flip. Negating the whole
        // comparison would drag them to the top on a descending sort, burying
        // the rows the user can actually read.
        const aNull = !a || a.kind === 'null';
        const bNull = !b || b.kind === 'null';
        if (aNull || bNull) return aNull && bNull ? 0 : aNull ? 1 : -1;

        const c = compareValues(a, b);
        return sort.desc ? -c : c;
      });
    }
  }

  return { ...result, rows };
}

/** Cycle a column through ascending → descending → unsorted. */
export function nextSort(current: SortState | null, column: string): SortState | null {
  if (current?.column !== column) return { column, desc: false };
  if (!current.desc) return { column, desc: true };
  return null;
}
