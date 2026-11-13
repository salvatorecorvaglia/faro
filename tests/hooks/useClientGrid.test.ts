import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useClientGrid } from '@/features/results/useClientGrid';
import type { ResultSet } from '@/ipc/types';

class WorkerMock {
  static instances: WorkerMock[] = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: ((event: { message: string }) => void) | null = null;
  postMessage = vi.fn();
  terminate = vi.fn();
  constructor() {
    WorkerMock.instances.push(this);
  }
}
const result: ResultSet = {
  columns: [{ name: 'id', typeName: 'int' }],
  rows: Array.from({ length: 3000 }, (_, i) => [{ kind: 'int', value: i }]),
  truncated: false,
  elapsedMs: 0,
};
afterEach(() => {
  vi.unstubAllGlobals();
  WorkerMock.instances = [];
});

it('ignores stale worker replies, reuses the loaded page, and terminates on unmount', async () => {
  vi.stubGlobal('Worker', WorkerMock);
  const filters: [] = [];
  const ascending = { column: 'id', desc: false };
  const descending = { column: 'id', desc: true };
  const {
    result: hook,
    rerender,
    unmount,
  } = renderHook(
    ({ desc }) => useClientGrid(result, true, desc ? descending : ascending, filters),
    {
      initialProps: { desc: false },
    },
  );
  await waitFor(() => expect(WorkerMock.instances).toHaveLength(1));
  const worker = WorkerMock.instances[0]!;
  const first = worker.postMessage.mock.calls[0]![0];
  rerender({ desc: true });
  const latest = worker.postMessage.mock.calls.at(-1)![0];
  expect(latest.result).toBeUndefined();
  act(() => worker.onmessage!({ data: { id: first.id, indices: [0] } }));
  expect(hook.current.pending).toBe(true);
  act(() => worker.onmessage!({ data: { id: latest.id, indices: [2999, 2998] } }));
  expect(hook.current.rows!.rows[0]).toBe(result.rows[2999]);
  expect(hook.current.pending).toBe(false);
  unmount();
  expect(worker.terminate).toHaveBeenCalledOnce();
});

it('surfaces worker failures rather than presenting unsorted rows as a completed sort', async () => {
  vi.stubGlobal('Worker', WorkerMock);
  const sort = { column: 'id', desc: false };
  const filters: [] = [];
  const { result: hook } = renderHook(() => useClientGrid(result, true, sort, filters));
  await waitFor(() => expect(WorkerMock.instances).toHaveLength(1));
  act(() => WorkerMock.instances[0]!.onerror!({ message: 'Worker failed' }));
  expect(hook.current.error).toBe('Worker failed');
  expect(hook.current.pending).toBe(false);
});
