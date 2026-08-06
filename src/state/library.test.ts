import { describe, expect, it } from 'vitest';

import type { SavedQuery } from '@/ipc/types';
import { folderNames, groupByFolder } from './library';

const q = (name: string, folder: string | null): SavedQuery => ({
  id: name,
  name,
  folder,
  sql: 'SELECT 1',
  connectionId: null,
  createdAt: '',
  updatedAt: '',
});

describe('groupByFolder', () => {
  it('groups consecutive queries sharing a folder', () => {
    // The backend already orders by folder, so grouping is a single pass.
    const out = groupByFolder([q('a', 'Reports'), q('b', 'Reports'), q('c', null)]);
    expect(out).toHaveLength(2);
    expect(out[0]!.folder).toBe('Reports');
    expect(out[0]!.queries.map((x) => x.name)).toEqual(['a', 'b']);
    expect(out[1]!.folder).toBeNull();
  });

  it('keeps a folder that reappears later as its own group', () => {
    // Guards against silently merging non-adjacent runs, which would reorder
    // what the backend deliberately sorted.
    const out = groupByFolder([q('a', 'X'), q('b', 'Y'), q('c', 'X')]);
    expect(out.map((g) => g.folder)).toEqual(['X', 'Y', 'X']);
  });

  it('returns nothing for an empty list', () => {
    expect(groupByFolder([])).toEqual([]);
  });

  it('puts every loose query in one null group', () => {
    const out = groupByFolder([q('a', null), q('b', null)]);
    expect(out).toHaveLength(1);
    expect(out[0]!.queries).toHaveLength(2);
  });
});

describe('folderNames', () => {
  it('returns distinct folders, sorted', () => {
    const out = folderNames([q('a', 'Zed'), q('b', 'Alpha'), q('c', 'Zed'), q('d', null)]);
    expect(out).toEqual(['Alpha', 'Zed']);
  });

  it('is empty when nothing is foldered', () => {
    expect(folderNames([q('a', null)])).toEqual([]);
  });
});
