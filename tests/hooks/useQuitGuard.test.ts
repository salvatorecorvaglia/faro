import { listen } from '@tauri-apps/api/event';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useQuitGuard } from '@/hooks/useQuitGuard';
import { useConfirmStore } from '@/state/confirm';
import { useConnections } from '@/state/connections';
import { useTabs } from '@/state/tabs';
import { callsTo, mockInvoke } from '@/test/ipc';

beforeEach(() => {
  vi.clearAllMocks();
  useTabs.setState({ tabs: [], activeId: null });
  useConfirmStore.setState({ request: null });
  mockInvoke({ set_exit_guard: () => {}, approve_exit: () => {} });
});

async function quit(running = false) {
  const call = vi.mocked(listen).mock.calls.find(([event]) => event === 'faro://quit-requested');
  if (!call) throw new Error('quit listener missing');
  // Dispatch like the native event bridge. The listener continues asynchronously.
  act(() => {
    call[1]({ event: 'faro://quit-requested', id: 1, payload: running });
  });
}

it('registers before enabling native interception and cleans up on unmount', async () => {
  const unlisten = vi.fn();
  vi.mocked(listen).mockResolvedValueOnce(unlisten);
  const { unmount } = renderHook(useQuitGuard);
  await waitFor(() => expect(callsTo('set_exit_guard')).toEqual([{ ready: true }]));
  unmount();
  await waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
  expect(callsTo('set_exit_guard')).toEqual([{ ready: true }, { ready: false }]);
});

it('declining native quit preserves staged edits and never approves exit', async () => {
  const id = useTabs.getState().openTableTab('c1', { schema: null, name: 'items' });
  useTabs.getState().update(id, { dirty: true });
  renderHook(useQuitGuard);
  await waitFor(() => expect(callsTo('set_exit_guard')).toHaveLength(1));
  await quit();
  expect(useConfirmStore.getState().request?.message).toMatch(/unapplied edits/);
  await act(async () => {
    useConfirmStore.getState().request?.resolve(false);
  });
  expect(callsTo('approve_exit')).toHaveLength(0);
  expect(useTabs.getState().tabs[0]?.dirty).toBe(true);
});

it('coalesces repeated close requests and warns about operations before approving', async () => {
  renderHook(useQuitGuard);
  await waitFor(() => expect(callsTo('set_exit_guard')).toHaveLength(1));
  await quit(true);
  const request = useConfirmStore.getState().request;
  expect(request?.message).toMatch(/operations are still running/);
  await quit(true);
  expect(useConfirmStore.getState().request).toBe(request);
  await act(async () => {
    request?.resolve(true);
  });
  await waitFor(() => expect(callsTo('approve_exit')).toHaveLength(1));
});

it('approves an idle, clean quit without a confirmation', async () => {
  renderHook(useQuitGuard);
  await waitFor(() => expect(callsTo('set_exit_guard')).toHaveLength(1));
  await quit();
  await waitFor(() => expect(callsTo('approve_exit')).toHaveLength(1));
  expect(useConfirmStore.getState().request).toBeNull();
});

it('surfaces an exit-command failure while preserving dirty state', async () => {
  const id = useTabs.getState().openTableTab('c1', { schema: null, name: 'items' });
  useTabs.getState().update(id, { dirty: true });
  mockInvoke({
    set_exit_guard: () => {},
    approve_exit: () => {
      throw { kind: 'other', message: 'Exit failed' };
    },
  });
  renderHook(useQuitGuard);
  await waitFor(() => expect(callsTo('set_exit_guard')).toHaveLength(1));
  await quit();
  await act(async () => {
    useConfirmStore.getState().request?.resolve(true);
  });
  await waitFor(() => expect(useConnections.getState().error).toBe('Exit failed'));
  expect(useTabs.getState().tabs[0]?.dirty).toBe(true);
});
