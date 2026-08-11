import { describe, expect, it } from 'vitest';

import type { ResultSet, Value } from '@/ipc/types';
import { applyGridOps, compareValues, matchesFilter, nextSort } from '@/lib/grid';

const int = (n: number): Value => ({ kind: 'int', value: n });
const text = (s: string): Value => ({ kind: 'text', value: s });
const nul: Value = { kind: 'null' };

function makeResult(rows: Value[][]): ResultSet {
  return {
    columns: [
      { name: 'id', typeName: 'int4' },
      { name: 'name', typeName: 'text' },
    ],
    rows,
    truncated: false,
    elapsedMs: 1,
  };
}

describe('compareValues', () => {
  it('orders numbers numerically, not as text', () => {
    // The classic bug: "10" < "9" under string comparison.
    expect(compareValues(int(9), int(10))).toBeLessThan(0);
    expect(compareValues(int(100), int(20))).toBeGreaterThan(0);
  });

  it('sorts NULL last in both directions', () => {
    // Ascending puts NULL at the end...
    expect(compareValues(nul, int(1))).toBeGreaterThan(0);
    expect(compareValues(int(1), nul)).toBeLessThan(0);
    expect(compareValues(nul, nul)).toBe(0);
  });

  it('compares decimals by value despite being strings', () => {
    const a: Value = { kind: 'decimal', value: '9.5' };
    const b: Value = { kind: 'decimal', value: '10.1' };
    expect(compareValues(a, b)).toBeLessThan(0);
  });

  it('orders booleans false before true', () => {
    const f: Value = { kind: 'bool', value: false };
    const t: Value = { kind: 'bool', value: true };
    expect(compareValues(f, t)).toBeLessThan(0);
  });

  it('sorts text case-insensitively and near accented forms', () => {
    expect(compareValues(text('apple'), text('Banana'))).toBeLessThan(0);
    expect(compareValues(text('Ölafur'), text('Zed'))).toBeLessThan(0);
  });
});

describe('matchesFilter', () => {
  it('matches contains case-insensitively', () => {
    expect(matchesFilter(text('Hello World'), 'contains', 'hello')).toBe(true);
    expect(matchesFilter(text('Hello'), 'contains', 'xyz')).toBe(false);
  });

  it('handles null checks independently of the needle', () => {
    expect(matchesFilter(nul, 'isNull', '')).toBe(true);
    expect(matchesFilter(text('a'), 'isNull', '')).toBe(false);
    expect(matchesFilter(nul, 'isNotNull', '')).toBe(false);
    expect(matchesFilter(text('a'), 'isNotNull', '')).toBe(true);
  });

  it('never matches a NULL cell against a value filter', () => {
    expect(matchesFilter(nul, 'contains', 'a')).toBe(false);
    expect(matchesFilter(nul, 'equals', '')).toBe(false);
  });

  it('compares numerically for greater/less than', () => {
    expect(matchesFilter(int(10), 'greaterThan', '9')).toBe(true);
    expect(matchesFilter(int(10), 'lessThan', '9')).toBe(false);
  });

  it('falls back to text ordering on non-numeric comparisons', () => {
    expect(matchesFilter(text('m'), 'greaterThan', 'a')).toBe(true);
    expect(matchesFilter(text('a'), 'greaterThan', 'm')).toBe(false);
  });
});

describe('applyGridOps', () => {
  const result = makeResult([
    [int(3), text('charlie')],
    [int(1), text('alpha')],
    [int(2), nul],
  ]);

  it('returns the identical object when there is nothing to do', () => {
    // Identity matters: the grid memoizes on it to skip re-renders.
    expect(applyGridOps(result, null, [])).toBe(result);
  });

  it('sorts ascending and descending', () => {
    const asc = applyGridOps(result, { column: 'id', desc: false }, []);
    expect(asc.rows.map((r) => (r[0] as { value: number }).value)).toEqual([1, 2, 3]);

    const desc = applyGridOps(result, { column: 'id', desc: true }, []);
    expect(desc.rows.map((r) => (r[0] as { value: number }).value)).toEqual([3, 2, 1]);
  });

  it('does not mutate the original rows', () => {
    const before = result.rows.map((r) => (r[0] as { value: number }).value);
    applyGridOps(result, { column: 'id', desc: true }, []);
    const after = result.rows.map((r) => (r[0] as { value: number }).value);
    expect(after).toEqual(before);
  });

  it('keeps NULL last even when sorting descending', () => {
    const desc = applyGridOps(result, { column: 'name', desc: true }, []);
    expect(desc.rows[desc.rows.length - 1]![1]!.kind).toBe('null');
  });

  it('filters rows', () => {
    const out = applyGridOps(result, null, [{ column: 'name', op: 'contains', value: 'alph' }]);
    expect(out.rows).toHaveLength(1);
  });

  it('ignores a filter with an empty value', () => {
    // An empty filter box should not hide every row.
    const out = applyGridOps(result, null, [{ column: 'name', op: 'contains', value: '' }]);
    expect(out.rows).toHaveLength(3);
  });

  it('still applies isNull with an empty value', () => {
    const out = applyGridOps(result, null, [{ column: 'name', op: 'isNull', value: '' }]);
    expect(out.rows).toHaveLength(1);
  });

  it('ANDs multiple filters', () => {
    const out = applyGridOps(result, null, [
      { column: 'id', op: 'greaterThan', value: '1' },
      { column: 'name', op: 'isNotNull', value: '' },
    ]);
    expect(out.rows).toHaveLength(1);
    expect((out.rows[0]![0] as { value: number }).value).toBe(3);
  });

  it('ignores filters on columns that are gone', () => {
    const out = applyGridOps(result, null, [{ column: 'vanished', op: 'equals', value: 'x' }]);
    expect(out.rows).toHaveLength(3);
  });

  it('filters before sorting', () => {
    const out = applyGridOps(result, { column: 'id', desc: false }, [
      { column: 'name', op: 'isNotNull', value: '' },
    ]);
    expect(out.rows.map((r) => (r[0] as { value: number }).value)).toEqual([1, 3]);
  });
});

describe('nextSort', () => {
  it('cycles ascending, descending, then off', () => {
    let s = nextSort(null, 'id');
    expect(s).toEqual({ column: 'id', desc: false });
    s = nextSort(s, 'id');
    expect(s).toEqual({ column: 'id', desc: true });
    s = nextSort(s, 'id');
    expect(s).toBeNull();
  });

  it('restarts at ascending when a different column is clicked', () => {
    const s = nextSort({ column: 'id', desc: true }, 'name');
    expect(s).toEqual({ column: 'name', desc: false });
  });
});
