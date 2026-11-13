import { listen } from '@tauri-apps/api/event';
import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useAsyncAction, useBackendProgress } from '@/hooks/useAsyncAction';

const listenMock = vi.mocked(listen);

beforeEach(() => {
  vi.clearAllMocks();
});

describe('useAsyncAction', () => {
  it('tracks busy across the action and reports its message', async () => {
    const { result } = renderHook(() => useAsyncAction());
    expect(result.current.busy).toBe(false);

    let finish: (v: string) => void = () => {};
    const pending = new Promise<string>((resolve) => {
      finish = resolve;
    });

    let ran: Promise<boolean>;
    act(() => {
      ran = result.current.run(() => pending);
    });
    await waitFor(() => expect(result.current.busy).toBe(true));

    await act(async () => {
      finish('Exported 5 rows');
      await ran;
    });

    expect(result.current.busy).toBe(false);
    expect(result.current.done).toBe('Exported 5 rows');
    expect(result.current.error).toBeNull();
  });

  it('records a failure and reports it as unsuccessful', async () => {
    const { result } = renderHook(() => useAsyncAction());

    let outcome: boolean | undefined;
    await act(async () => {
      outcome = await result.current.run(async () => {
        throw { kind: 'io', message: 'disk full' };
      });
    });

    expect(outcome).toBe(false);
    expect(result.current.error).toBe('disk full');
    expect(result.current.busy).toBe(false);
  });

  it('leaves `done` unset when the action returns nothing', async () => {
    // Dialogs that render their own success UI return void rather than a
    // message, and must not get an empty one-liner shown above it.
    const { result } = renderHook(() => useAsyncAction());
    await act(async () => {
      await result.current.run(async () => {});
    });
    expect(result.current.done).toBeNull();
  });

  it('clears a previous outcome when reset', async () => {
    const { result } = renderHook(() => useAsyncAction());
    await act(async () => {
      await result.current.run(async () => 'ok');
    });
    expect(result.current.done).toBe('ok');

    act(() => result.current.reset());
    expect(result.current.done).toBeNull();
    expect(result.current.error).toBeNull();
  });
});

describe('useBackendProgress', () => {
  it('does not subscribe while inactive', () => {
    renderHook(() => useBackendProgress('faro://backup-progress', false));
    expect(listenMock).not.toHaveBeenCalled();
  });

  it('subscribes when active and surfaces the payload', async () => {
    let emit: (e: { payload: { queryId: string; progress: { done: number } } }) => void = () => {};
    listenMock.mockImplementation(async (_event, handler) => {
      emit = handler as typeof emit;
      return () => {};
    });

    const { result } = renderHook(() =>
      useBackendProgress<{ done: number }>('faro://backup-progress', true, 'test-operation'),
    );
    await waitFor(() =>
      expect(listenMock).toHaveBeenCalledWith('faro://backup-progress', expect.any(Function)),
    );

    act(() => emit({ payload: { queryId: 'test-operation', progress: { done: 3 } } }));
    expect(result.current).toEqual({ done: 3 });
  });

  it('unsubscribes when it goes inactive, so a finished backup stops listening', async () => {
    const unlisten = vi.fn();
    listenMock.mockImplementation(async () => unlisten);

    const { rerender } = renderHook(
      ({ active }) => useBackendProgress('faro://backup-progress', active, 'test-operation'),
      { initialProps: { active: true } },
    );
    await waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    rerender({ active: false });
    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });

  it('unsubscribes on unmount', async () => {
    const unlisten = vi.fn();
    listenMock.mockImplementation(async () => unlisten);

    const { unmount } = renderHook(() => useBackendProgress('faro://restore-progress', true));
    await waitFor(() => expect(listenMock).toHaveBeenCalledTimes(1));

    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });

  it('drops stale progress when it goes inactive', async () => {
    let emit: (e: { payload: { queryId: string; progress: { done: number } } }) => void = () => {};
    listenMock.mockImplementation(async (_event, handler) => {
      emit = handler as typeof emit;
      return () => {};
    });

    const { result, rerender } = renderHook(
      ({ active }) =>
        useBackendProgress<{ done: number }>('faro://backup-progress', active, 'test-operation'),
      { initialProps: { active: true } },
    );
    await waitFor(() => expect(listenMock).toHaveBeenCalled());
    act(() => emit({ payload: { queryId: 'test-operation', progress: { done: 7 } } }));
    expect(result.current).toEqual({ done: 7 });

    // Reopening the dialog must not show the previous run's progress.
    rerender({ active: false });
    expect(result.current).toBeNull();
  });
});

it('ignores progress for other operations and discards queued events after switching IDs', async () => {
  const events: Array<
    (event: { payload: { queryId: string; progress: { done: number } } }) => void
  > = [];
  listenMock.mockImplementation(async (_name, handler) => {
    events.push(handler as (typeof events)[number]);
    return () => {};
  });
  const { result, rerender } = renderHook(
    ({ id }) => useBackendProgress<{ done: number }>('faro://backup-progress', true, id),
    { initialProps: { id: 'one' } },
  );
  await waitFor(() => expect(events).toHaveLength(1));
  act(() => events[0]?.({ payload: { queryId: 'other', progress: { done: 99 } } }));
  expect(result.current).toBeNull();
  act(() => events[0]?.({ payload: { queryId: 'one', progress: { done: 1 } } }));
  expect(result.current).toEqual({ done: 1 });
  rerender({ id: 'two' });
  await waitFor(() => expect(events).toHaveLength(2));
  act(() => events[0]?.({ payload: { queryId: 'one', progress: { done: 99 } } }));
  expect(result.current).toBeNull();
  act(() => events[1]?.({ payload: { queryId: 'two', progress: { done: 2 } } }));
  expect(result.current).toEqual({ done: 2 });
});

it('refuses overlapping backend actions within the same tick', async () => {
  const { result } = renderHook(() => useAsyncAction());
  let finish!: () => void;
  const first = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  const second = vi.fn(async () => {});
  let pending!: Promise<boolean>;
  await act(async () => {
    pending = result.current.run(first);
    expect(await result.current.run(second)).toBe(false);
  });
  expect(second).not.toHaveBeenCalled();
  expect(result.current.busy).toBe(true);
  await act(async () => {
    finish();
    await pending;
  });
  expect(result.current.busy).toBe(false);
  await act(async () => {
    expect(await result.current.run(second)).toBe(true);
  });
});
