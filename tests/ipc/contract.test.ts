import { describe, expect, it } from 'vitest';
import { toFaroError } from '@/ipc';
import type { PendingChange, ResultSet } from '@/ipc/types';
import { emptyEdits, setCell, toPendingChanges } from '@/lib/edits';
import { formatValue, toJson } from '@/lib/value';
import fixture from './fixtures/contract.json';

describe('Rust IPC serialization fixture', () => {
  it('preserves exact integers, binary descriptors, nested values and nulls in the UI', () => {
    const result = fixture.result as ResultSet;
    expect(result.columns[0]?.typeName).toBe('fixture');
    expect(result.elapsedMs).toBe(7);
    expect(result.rows[0]!.map(formatValue)).toEqual([
      '9223372036854775807',
      '[2 B]',
      '[257 B]',
      '{, 123.4500}',
      '',
    ]);
    expect(toJson(result.rows[0]![0]!)).toBe('9223372036854775807');
  });

  it('recognizes serialized backend errors without losing database codes', () => {
    for (const error of fixture.errors) expect(toFaroError(error)).toEqual(error);
  });

  it('sends optimistic edit snapshots in the shape Rust deserializes', () => {
    const result: ResultSet = {
      columns: [
        { name: 'id', typeName: 'bigint' },
        { name: 'name', typeName: 'text' },
      ],
      rows: [
        [
          { kind: 'decimal', value: '9223372036854775807' },
          { kind: 'text', value: 'old' },
        ],
      ],
      truncated: false,
      elapsedMs: 0,
    };
    const edits = setCell(
      emptyEdits(),
      0,
      'name',
      { kind: 'text', value: 'new' },
      result.rows[0]![1],
    );
    const changes: PendingChange[] = toPendingChanges(result, ['id'], edits);
    expect(changes).toEqual(fixture.changes);
  });
});
