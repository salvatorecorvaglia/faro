import type { ResultSet } from '@/ipc/types';
import { formatValue } from '@/lib/value';

/**
 * Fixed metrics for the result grid.
 *
 * Shared by the grid and its cells, and named here rather than in either so
 * neither owns geometry the other depends on — the row height in particular has
 * to match between the virtualizer's `estimateSize` and the inline cell editor,
 * or an open editor sits a pixel off the row it belongs to.
 */
export const ROW_HEIGHT = 26;
export const HEADER_HEIGHT = 28;
export const FILTER_HEIGHT = 26;
export const ROW_NUM_WIDTH = 52;
export const MIN_COL_WIDTH = 56;

/**
 * Pick a width per column from the header plus a sample of rows.
 *
 * Sampling the first 100 rows keeps this cheap while still sizing sensibly;
 * measuring every row of a large result would cost more than rendering it.
 */
export function measureWidths(result: ResultSet): Record<string, number> {
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
