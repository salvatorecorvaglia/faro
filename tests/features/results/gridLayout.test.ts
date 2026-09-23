import { describe, expect, it } from 'vitest';
import {
  DEFAULT_COL_WIDTH,
  MIN_COL_WIDTH,
  measureWidths,
  ROW_HEIGHT,
} from '@/features/results/gridLayout';
import type { ColumnInfo, ResultSet, Value } from '@/ipc/types';

const col = (name: string, typeName = 'text'): ColumnInfo => ({ name, typeName });
const text = (s: string): Value => ({ kind: 'text', value: s });

function result(columns: ColumnInfo[], rows: Value[][]): ResultSet {
  return { columns, rows, truncated: false, elapsedMs: 0 };
}

/**
 * Column sizing is what makes the grid readable, and it had no direct test —
 * every assertion about it went through `ResultGrid`, where the jsdom layout
 * stubs make real geometry unobservable anyway.
 */
describe('measureWidths', () => {
  it('keys widths by name and position, so duplicate column names do not collide', () => {
    // SQL permits `SELECT a.id, b.id`; keying on the name alone would size and
    // resize both together.
    const widths = measureWidths(result([col('id'), col('id')], []));
    expect(Object.keys(widths)).toHaveLength(2);
  });

  it('never sizes a column below the minimum', () => {
    const widths = measureWidths(result([col('a')], [[text('')]]));
    for (const w of Object.values(widths)) expect(w).toBeGreaterThanOrEqual(MIN_COL_WIDTH);
  });

  it('caps a very long value rather than letting one cell define the layout', () => {
    const widths = measureWidths(result([col('bio')], [[text('x'.repeat(5000))]]));
    // Without a cap, a single long value pushes every other column off screen.
    for (const w of Object.values(widths)) expect(w).toBeLessThanOrEqual(420);
  });

  it('sizes from the widest sampled value, not the first', () => {
    const narrow = measureWidths(result([col('a')], [[text('x')]]));
    const wide = measureWidths(result([col('a')], [[text('x')], [text('a much longer value')]]));
    expect(Object.values(wide)[0]).toBeGreaterThan(Object.values(narrow)[0] as number);
  });

  it('samples a bounded number of rows', () => {
    // Measuring every row of a large result costs more than rendering it, so
    // only the first hundred are read — a wide value past that is not sized for.
    const rows: Value[][] = Array.from({ length: 200 }, () => [text('x')]);
    rows[150] = [text('y'.repeat(300))];
    const widths = measureWidths(result([col('a')], rows));
    const sampledOnly = measureWidths(result([col('a')], rows.slice(0, 100)));
    expect(Object.values(widths)[0]).toBe(Object.values(sampledOnly)[0]);
  });

  it('accounts for the header, which is a name and a type', () => {
    const short = measureWidths(result([col('a', 'int4')], []));
    const long = measureWidths(result([col('a_very_long_column_name', 'timestamptz')], []));
    expect(Object.values(long)[0]).toBeGreaterThan(Object.values(short)[0] as number);
  });

  it('renders NULL at a readable width rather than as nothing', () => {
    const widths = measureWidths(result([col('a')], [[{ kind: 'null' }]]));
    expect(Object.values(widths)[0]).toBeGreaterThanOrEqual(MIN_COL_WIDTH);
  });
});

describe('the fixed metrics', () => {
  it('keeps the fallback width within the grid’s own bounds', () => {
    // `widthOf` falls back to this between a new result arriving and the
    // layout effect re-measuring, so it has to be a width the grid accepts.
    expect(DEFAULT_COL_WIDTH).toBeGreaterThanOrEqual(MIN_COL_WIDTH);
    expect(ROW_HEIGHT).toBeGreaterThan(0);
  });
});
