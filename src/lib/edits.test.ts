import { describe, expect, it } from 'vitest';

import type { ColumnDetail, ResultSet, Value } from '@/ipc/types';
import {
  addRow,
  changeCount,
  emptyEdits,
  hasChanges,
  removeInsert,
  setCell,
  setInsertCell,
  toggleDelete,
  toPendingChanges,
} from './edits';

const int = (n: number): Value => ({ kind: 'int', value: n });
const text = (s: string): Value => ({ kind: 'text', value: s });
const nul: Value = { kind: 'null' };

const result: ResultSet = {
  columns: [
    { name: 'id', typeName: 'int4' },
    { name: 'name', typeName: 'text' },
    { name: 'bio', typeName: 'text' },
  ],
  rows: [
    [int(1), text('Ada'), nul],
    [int(2), text('Grace'), text('hi')],
  ],
  truncated: false,
  elapsedMs: 1,
};

const columns: ColumnDetail[] = [
  { name: 'id', typeName: 'int4', nullable: false, default: null, isPrimaryKey: true, ordinal: 0 },
  {
    name: 'name',
    typeName: 'text',
    nullable: false,
    default: null,
    isPrimaryKey: false,
    ordinal: 1,
  },
];

describe('staging', () => {
  it('starts with nothing staged', () => {
    const e = emptyEdits();
    expect(hasChanges(e)).toBe(false);
    expect(changeCount(e)).toBe(0);
  });

  it('stages a cell edit', () => {
    const e = setCell(emptyEdits(), 0, 'name', { kind: 'text', value: 'Ada L' }, text('Ada'));
    expect(hasChanges(e)).toBe(true);
    expect(e.updates[0]!.name).toEqual({ kind: 'text', value: 'Ada L' });
  });

  it('drops an edit that returns the original value', () => {
    // Otherwise Apply would run an UPDATE that changes nothing.
    let e = setCell(emptyEdits(), 0, 'name', { kind: 'text', value: 'Changed' }, text('Ada'));
    e = setCell(e, 0, 'name', { kind: 'text', value: 'Ada' }, text('Ada'));
    expect(hasChanges(e)).toBe(false);
    expect(e.updates[0]).toBeUndefined();
  });

  it('treats NULL and the empty string as different edits', () => {
    // An emptied cell is ambiguous, so the two must never collapse together.
    const toNull = setCell(emptyEdits(), 1, 'bio', { kind: 'null' }, text('hi'));
    const toEmpty = setCell(emptyEdits(), 1, 'bio', { kind: 'text', value: '' }, text('hi'));
    expect(hasChanges(toNull)).toBe(true);
    expect(hasChanges(toEmpty)).toBe(true);
    expect(toNull.updates[1]!.bio).not.toEqual(toEmpty.updates[1]!.bio);
  });

  it('clears a NULL edit when the cell was already NULL', () => {
    const e = setCell(emptyEdits(), 0, 'bio', { kind: 'null' }, nul);
    expect(hasChanges(e)).toBe(false);
  });

  it('does not treat typed text as matching an original NULL', () => {
    // NULL renders as empty, so a naive comparison would wrongly cancel this.
    const e = setCell(emptyEdits(), 0, 'bio', { kind: 'text', value: '' }, nul);
    expect(hasChanges(e)).toBe(true);
  });

  it('toggles a row deletion on and off', () => {
    let e = toggleDelete(emptyEdits(), 1);
    expect(e.deletes).toEqual([1]);
    e = toggleDelete(e, 1);
    expect(e.deletes).toEqual([]);
  });

  it('adds a blank row with every column defaulted', () => {
    const e = addRow(emptyEdits(), columns);
    expect(e.inserts).toHaveLength(1);
    // Defaults let an auto-increment key work without the user inventing one.
    expect(e.inserts[0]!.id).toEqual({ kind: 'default' });
  });

  it('edits and removes a staged insert', () => {
    let e = addRow(emptyEdits(), columns);
    e = setInsertCell(e, 0, 'name', { kind: 'text', value: 'New' });
    expect(e.inserts[0]!.name).toEqual({ kind: 'text', value: 'New' });

    e = removeInsert(e, 0);
    expect(e.inserts).toHaveLength(0);
  });

  it('counts each affected row once', () => {
    let e = setCell(emptyEdits(), 0, 'name', { kind: 'text', value: 'x' }, text('Ada'));
    e = setCell(e, 0, 'bio', { kind: 'text', value: 'y' }, nul);
    e = toggleDelete(e, 1);
    e = addRow(e, columns);
    // Two cells on one row is still one changed row.
    expect(changeCount(e)).toBe(3);
  });
});

describe('toPendingChanges', () => {
  it('produces an update keyed on the original primary key', () => {
    const e = setCell(emptyEdits(), 0, 'name', { kind: 'text', value: 'Ada L' }, text('Ada'));
    const changes = toPendingChanges(result, ['id'], e);

    expect(changes).toHaveLength(1);
    expect(changes[0]).toEqual({
      type: 'update',
      key: [{ column: 'id', value: { kind: 'text', value: '1' } }],
      cells: [{ column: 'name', value: { kind: 'text', value: 'Ada L' } }],
    });
  });

  it('keys on the original value even when the key itself is edited', () => {
    // Otherwise the UPDATE would search for a row that does not exist yet.
    const e = setCell(emptyEdits(), 0, 'id', { kind: 'text', value: '99' }, int(1));
    const changes = toPendingChanges(result, ['id'], e);

    const update = changes[0] as { key: { value: { value: string } }[] };
    expect(update.key[0]!.value.value).toBe('1');
  });

  it('emits only the delete for a row that is both edited and deleted', () => {
    let e = setCell(emptyEdits(), 1, 'name', { kind: 'text', value: 'x' }, text('Grace'));
    e = toggleDelete(e, 1);

    const changes = toPendingChanges(result, ['id'], e);
    expect(changes).toHaveLength(1);
    expect(changes[0]!.type).toBe('delete');
  });

  it('carries every column of a composite key', () => {
    const composite: ResultSet = {
      columns: [
        { name: 'a', typeName: 'int4' },
        { name: 'b', typeName: 'int4' },
        { name: 'v', typeName: 'int4' },
      ],
      rows: [[int(1), int(2), int(3)]],
      truncated: false,
      elapsedMs: 1,
    };
    const e = setCell(emptyEdits(), 0, 'v', { kind: 'text', value: '9' }, int(3));
    const changes = toPendingChanges(composite, ['a', 'b'], e);

    const update = changes[0] as { key: { column: string }[] };
    expect(update.key.map((k) => k.column)).toEqual(['a', 'b']);
  });

  it('represents an original NULL key as NULL, not empty text', () => {
    const withNull: ResultSet = {
      columns: [
        { name: 'id', typeName: 'int4' },
        { name: 'v', typeName: 'text' },
      ],
      rows: [[nul, text('x')]],
      truncated: false,
      elapsedMs: 1,
    };
    const e = setCell(emptyEdits(), 0, 'v', { kind: 'text', value: 'y' }, text('x'));
    const changes = toPendingChanges(withNull, ['id'], e);

    const update = changes[0] as { key: { value: { kind: string } }[] };
    expect(update.key[0]!.value.kind).toBe('null');
  });

  it('emits inserts for staged rows', () => {
    let e = addRow(emptyEdits(), columns);
    e = setInsertCell(e, 0, 'name', { kind: 'text', value: 'New' });

    const changes = toPendingChanges(result, ['id'], e);
    expect(changes).toHaveLength(1);
    expect(changes[0]!.type).toBe('insert');
  });

  it('returns nothing when nothing is staged', () => {
    expect(toPendingChanges(result, ['id'], emptyEdits())).toEqual([]);
  });
});
