import { renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useUpdater } from '@/hooks/useUpdater';
import { useUpdaterStore } from '@/state/updater';

/**
 * The delay and the once-only guard are the whole hook, and neither was
 * covered: a regression in either means either an update check competing with
 * the app's own startup work, or one firing repeatedly.
 */

const checkForUpdates = vi.fn(async () => {});

beforeEach(() => {
  vi.useFakeTimers();
  checkForUpdates.mockClear();
  useUpdaterStore.setState({ checkForUpdates });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useUpdater', () => {
  it('waits before checking, so startup is not competing with a download', () => {
    renderHook(() => useUpdater());

    expect(checkForUpdates).not.toHaveBeenCalled();
    vi.advanceTimersByTime(4999);
    expect(checkForUpdates).not.toHaveBeenCalled();

    vi.advanceTimersByTime(1);
    expect(checkForUpdates).toHaveBeenCalledOnce();
  });

  it('checks once per session, however often it re-renders', () => {
    const { rerender } = renderHook(() => useUpdater());
    rerender();
    rerender();

    vi.advanceTimersByTime(5000);
    expect(checkForUpdates).toHaveBeenCalledOnce();
  });

  it('does not check after unmounting', () => {
    // React 18+ mounts effects twice under StrictMode, and an unmount before
    // the timer fires must not leave one pending.
    const { unmount } = renderHook(() => useUpdater());
    unmount();

    vi.advanceTimersByTime(5000);
    expect(checkForUpdates).not.toHaveBeenCalled();
  });
});
