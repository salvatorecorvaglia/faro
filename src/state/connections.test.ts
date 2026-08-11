import { beforeEach, describe, expect, it } from 'vitest';

import { faroError, mockInvoke } from '@/test/ipc';
import { useConnections } from './connections';

const initial = useConnections.getState();

beforeEach(() => {
  useConnections.setState({
    ...initial,
    items: [],
    loading: true,
    connecting: new Set(),
    error: null,
    keychainOk: true,
  });
});

describe('useConnections', () => {
  it('loads connections and keychain availability', async () => {
    mockInvoke({
      list_connection_status: () => [{ id: 'a', name: 'A', connected: false }],
      keychain_available: () => true,
    });

    await useConnections.getState().refresh();

    const s = useConnections.getState();
    expect(s.items).toHaveLength(1);
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
  });

  it('surfaces a failed refresh instead of spinning forever', async () => {
    mockInvoke({ list_connection_status: faroError('store', 'settings are unreadable') });

    await useConnections.getState().refresh();

    const s = useConnections.getState();
    expect(s.loading).toBe(false);
    expect(s.error).toBe('settings are unreadable');
  });

  it('reports a failed connect and clears the spinner', async () => {
    mockInvoke({
      connect: faroError('connection', 'could not connect: bad password'),
      list_connection_status: () => [],
      keychain_available: () => true,
    });

    const ok = await useConnections.getState().connect('a');

    expect(ok).toBe(false);
    const s = useConnections.getState();
    expect(s.error).toBe('could not connect: bad password');
    expect(s.connecting.has('a')).toBe(false);
  });

  // The three below are the regression: these actions used to `await` an IPC
  // call with no catch, at call sites that also had none. A failure became an
  // unhandled promise rejection and the user saw nothing at all — including
  // when deleting a connection or wiping history.

  it('surfaces a failed disconnect rather than rejecting silently', async () => {
    mockInvoke({ disconnect: faroError('other', 'disconnect failed') });

    await expect(useConnections.getState().disconnect('a')).resolves.toBeUndefined();
    expect(useConnections.getState().error).toBe('disconnect failed');
  });

  it('surfaces a failed delete rather than rejecting silently', async () => {
    mockInvoke({ delete_connection: faroError('store', 'could not delete') });

    await expect(useConnections.getState().remove('a')).resolves.toBeUndefined();
    expect(useConnections.getState().error).toBe('could not delete');
  });

  it('surfaces a failed save rather than rejecting silently', async () => {
    mockInvoke({ save_connection: faroError('keychain', 'no keychain') });

    await expect(useConnections.getState().save({ id: 'a' } as never)).rejects.toBeDefined();
    expect(useConnections.getState().error).toBe('no keychain');
  });
});
