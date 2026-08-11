import { describe, expect, it } from 'vitest';

import type { ResultSet, Value } from '@/ipc/types';
import { formatDuration, formatValue, isNumeric, resultToJson, toJson } from '@/lib/value';

const text = (s: string): Value => ({ kind: 'text', value: s });

describe('formatValue', () => {
  it('renders NULL as empty so the cell styling can distinguish it', () => {
    // A literal "NULL" would be ambiguous with the string 'NULL'.
    expect(formatValue({ kind: 'null' })).toBe('');
    expect(formatValue(text('NULL'))).toBe('NULL');
  });

  it('keeps decimal precision as-is', () => {
    const big = '12345678901234567890.0987654321';
    expect(formatValue({ kind: 'decimal', value: big })).toBe(big);
  });

  it('summarizes bytes instead of dumping them into the grid', () => {
    const v: Value = { kind: 'bytes', value: new Array(2048).fill(0) };
    expect(formatValue(v)).toBe('[2.0 KB]');
  });

  it('renders arrays in brace notation', () => {
    const v: Value = { kind: 'array', value: [text('a'), text('b')] };
    expect(formatValue(v)).toBe('{a, b}');
  });

  it('labels unsupported values with their type name', () => {
    expect(formatValue({ kind: 'unsupported', value: 'geometry' })).toBe('[geometry]');
  });
});

describe('isNumeric', () => {
  it('treats decimals as numeric for alignment despite being strings', () => {
    expect(isNumeric({ kind: 'decimal', value: '1.5' })).toBe(true);
    expect(isNumeric({ kind: 'int', value: 1 })).toBe(true);
    expect(isNumeric(text('1'))).toBe(false);
  });
});

describe('toJson', () => {
  it('keeps decimals as strings to avoid float rounding', () => {
    // Number("12345678901234567890.1") would lose digits irrecoverably.
    const out = toJson({ kind: 'decimal', value: '12345678901234567890.1' });
    expect(out).toBe('12345678901234567890.1');
    expect(typeof out).toBe('string');
  });

  it('describes bytes rather than emitting a giant array', () => {
    expect(toJson({ kind: 'bytes', value: [1, 2, 3] })).toEqual({ $bytes: 3 });
  });

  it('passes JSON columns through unwrapped', () => {
    expect(toJson({ kind: 'json', value: { a: 1 } })).toEqual({ a: 1 });
  });

  it('maps SQL NULL to JSON null', () => {
    expect(toJson({ kind: 'null' })).toBeNull();
  });
});

describe('resultToJson', () => {
  it('builds one object per row keyed by column name', () => {
    const result: ResultSet = {
      columns: [
        { name: 'id', typeName: 'int4' },
        { name: 'name', typeName: 'text' },
      ],
      rows: [[{ kind: 'int', value: 1 }, text('ada')]],
      truncated: false,
      elapsedMs: 3,
    };
    expect(resultToJson(result)).toEqual([{ id: 1, name: 'ada' }]);
  });

  it('disambiguates duplicate column names instead of losing a column', () => {
    // `SELECT a.id, b.id FROM a JOIN b` is legal SQL but not a legal object.
    const result: ResultSet = {
      columns: [
        { name: 'id', typeName: 'int4' },
        { name: 'id', typeName: 'int4' },
      ],
      rows: [
        [
          { kind: 'int', value: 1 },
          { kind: 'int', value: 2 },
        ],
      ],
      truncated: false,
      elapsedMs: 1,
    };
    expect(resultToJson(result)).toEqual([{ id: 1, id_2: 2 }]);
  });
});

describe('formatDuration', () => {
  it('scales units with magnitude', () => {
    expect(formatDuration(42)).toBe('42 ms');
    expect(formatDuration(1500)).toBe('1.50 s');
    expect(formatDuration(65_000)).toBe('1m 5s');
  });
});
