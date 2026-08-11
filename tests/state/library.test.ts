import { beforeEach, describe, expect, it } from 'vitest';

import type { SavedQuery } from '@/ipc/types';
import { folderNames, groupByFolder, useLibrary } from '@/state/library';
import { faroError, mockInvoke } from '@/test/ipc';

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

describe('library write failures are surfaced', () => {
  // These used to `await` with no catch at call sites that also had none, so a
  // failure became an unhandled rejection and the user saw nothing.

  beforeEach(() => {
    useLibrary.setState({ error: null, saved: [], history: [] });
  });

  it('records a failed delete of a saved query', async () => {
    mockInvoke({ delete_saved_query: faroError('store', 'could not delete') });
    await expect(useLibrary.getState().remove('q1')).resolves.toBeUndefined();
    expect(useLibrary.getState().error).toBe('could not delete');
  });

  it('records a failed history wipe', async () => {
    mockInvoke({ clear_history: faroError('store', 'history is locked') });
    await expect(useLibrary.getState().clearHistory()).resolves.toBeUndefined();
    expect(useLibrary.getState().error).toBe('history is locked');
  });

  it('records a failed single history delete and keeps the row', async () => {
    useLibrary.setState({ history: [{ id: 1 } as never] });
    mockInvoke({ delete_history_entry: faroError('store', 'nope') });

    await expect(useLibrary.getState().deleteHistoryEntry(1)).resolves.toBeUndefined();

    expect(useLibrary.getState().error).toBe('nope');
    expect(useLibrary.getState().history).toHaveLength(1);
  });

  it('rethrows a failed save so the dialog stays open', async () => {
    mockInvoke({ save_query: faroError('store', 'disk full') });
    await expect(useLibrary.getState().save({ id: '', name: 'x' } as never)).rejects.toBeDefined();
    expect(useLibrary.getState().error).toBe('disk full');
  });

  it('leaves reads silent — the library is an accessory to the editor', async () => {
    mockInvoke({ list_saved_queries: faroError('store', 'unreadable') });
    await expect(useLibrary.getState().refreshSaved()).resolves.toBeUndefined();
    expect(useLibrary.getState().error).toBeNull();
  });
});
