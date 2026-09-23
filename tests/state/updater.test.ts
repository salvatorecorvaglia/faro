import { relaunch } from '@tauri-apps/plugin-process';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useUpdaterStore } from '@/state/updater';

/**
 * The updater downloads and executes a signed binary, and none of it was
 * tested: the store, the hook that starts it and the toast that drives it came
 * to 238 lines at under 9% coverage.
 */

/** A minimal stand-in for the plugin's `Update` handle. */
function fakeUpdate(overrides: Partial<Update> = {}): Update {
  return {
    available: true,
    version: '1.6.0',
    currentVersion: '1.5.0',
    downloadAndInstall: vi.fn(async () => {}),
    ...overrides,
  } as unknown as Update;
}

const initial = useUpdaterStore.getState();

beforeEach(() => {
  vi.clearAllMocks();
  useUpdaterStore.setState({
    ...initial,
    status: 'idle',
    version: null,
    progress: 0,
    error: null,
    updateRef: null,
    dismissed: false,
  });
});

afterEach(() => {
  vi.unstubAllEnvs();
});

describe('checkForUpdates', () => {
  it('does not contact the update server in development', async () => {
    vi.stubEnv('PROD', false);

    await useUpdaterStore.getState().checkForUpdates();

    // A dev build pointing at the release feed would offer to "update" a
    // working tree to the last published version.
    expect(check).not.toHaveBeenCalled();
    expect(useUpdaterStore.getState().status).toBe('idle');
  });

  it('offers an available update by version', async () => {
    vi.stubEnv('PROD', true);
    const update = fakeUpdate();
    vi.mocked(check).mockResolvedValue(update);

    await useUpdaterStore.getState().checkForUpdates();

    const s = useUpdaterStore.getState();
    expect(s.status).toBe('available');
    expect(s.version).toBe('1.6.0');
    expect(s.updateRef).toBe(update);
    // A fresh offer must not inherit an earlier dismissal.
    expect(s.dismissed).toBe(false);
  });

  it('clears a stale update handle when there is nothing to install', async () => {
    vi.stubEnv('PROD', true);
    useUpdaterStore.setState({ updateRef: fakeUpdate(), status: 'available' });
    vi.mocked(check).mockResolvedValue(null);

    await useUpdaterStore.getState().checkForUpdates();

    const s = useUpdaterStore.getState();
    expect(s.status).toBe('idle');
    expect(s.updateRef).toBeNull();
  });

  it('treats an unavailable update as nothing to install', async () => {
    vi.stubEnv('PROD', true);
    vi.mocked(check).mockResolvedValue(fakeUpdate({ available: false }));

    await useUpdaterStore.getState().checkForUpdates();

    expect(useUpdaterStore.getState().status).toBe('idle');
  });

  it('records a failed check without blocking the app', async () => {
    vi.stubEnv('PROD', true);
    vi.mocked(check).mockRejectedValue(new Error('could not reach the update server'));

    await useUpdaterStore.getState().checkForUpdates();

    const s = useUpdaterStore.getState();
    // Back to idle, not `error`: a failed *check* is not something the user
    // asked for, so it must not raise a toast — but it is still recorded.
    expect(s.status).toBe('idle');
    expect(s.error).toBe('could not reach the update server');
  });
});

describe('downloadAndInstall', () => {
  it('does nothing without an update to install', async () => {
    await useUpdaterStore.getState().downloadAndInstall();
    expect(useUpdaterStore.getState().status).toBe('idle');
  });

  it('reports progress as a percentage of the content length', async () => {
    const seen: number[] = [];
    const update = fakeUpdate({
      downloadAndInstall: vi.fn(async (onEvent: (e: unknown) => void) => {
        onEvent({ event: 'Started', data: { contentLength: 1000 } });
        onEvent({ event: 'Progress', data: { chunkLength: 250 } });
        seen.push(useUpdaterStore.getState().progress);
        onEvent({ event: 'Progress', data: { chunkLength: 750 } });
        seen.push(useUpdaterStore.getState().progress);
        onEvent({ event: 'Finished', data: {} });
      }),
    } as unknown as Partial<Update>);
    useUpdaterStore.setState({ updateRef: update });

    await useUpdaterStore.getState().downloadAndInstall();

    // Cumulative, not per-chunk: the bar has to move forward, not restart.
    expect(seen).toEqual([25, 100]);
    const s = useUpdaterStore.getState();
    expect(s.status).toBe('ready');
    expect(s.progress).toBe(100);
  });

  it('does not divide by a missing content length', async () => {
    // Some servers send no Content-Length. Reporting 0% is honest; NaN would
    // render as "NaN%" in the toast.
    const update = fakeUpdate({
      downloadAndInstall: vi.fn(async (onEvent: (e: unknown) => void) => {
        onEvent({ event: 'Started', data: {} });
        onEvent({ event: 'Progress', data: { chunkLength: 100 } });
      }),
    } as unknown as Partial<Update>);
    useUpdaterStore.setState({ updateRef: update });

    await useUpdaterStore.getState().downloadAndInstall();

    expect(useUpdaterStore.getState().status).toBe('ready');
  });

  it('surfaces a failed install as an error the user can see', async () => {
    const update = fakeUpdate({
      downloadAndInstall: vi.fn(async () => {
        throw new Error('signature verification failed');
      }),
    } as unknown as Partial<Update>);
    useUpdaterStore.setState({ updateRef: update });

    await useUpdaterStore.getState().downloadAndInstall();

    const s = useUpdaterStore.getState();
    // A rejected signature is exactly the case that must never look like
    // success — this is the path that runs a downloaded binary.
    expect(s.status).toBe('error');
    expect(s.error).toBe('signature verification failed');
  });
});

describe('restartApp', () => {
  it('reports a failed relaunch rather than appearing to hang', async () => {
    vi.mocked(relaunch).mockRejectedValue(new Error('relaunch is not permitted'));

    await useUpdaterStore.getState().restartApp();

    const s = useUpdaterStore.getState();
    expect(s.status).toBe('error');
    expect(s.error).toBe('relaunch is not permitted');
  });

  it('relaunches when asked', async () => {
    vi.mocked(relaunch).mockResolvedValue(undefined);
    await useUpdaterStore.getState().restartApp();
    expect(relaunch).toHaveBeenCalledOnce();
  });
});

describe('dismiss', () => {
  it('hides the toast without forgetting the update', () => {
    const update = fakeUpdate();
    useUpdaterStore.setState({ status: 'available', updateRef: update });

    useUpdaterStore.getState().dismiss();

    const s = useUpdaterStore.getState();
    expect(s.dismissed).toBe(true);
    // Still installable from wherever else the app offers it.
    expect(s.updateRef).toBe(update);
  });
});
