import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryTab } from '@/features/tabs/QueryTab';
import type { ConnectionStatus, RunResult } from '@/ipc/types';
import { useConnections } from '@/state/connections';
import { useSchemaCache } from '@/state/schemaCache';
import { type Tab, useTabs } from '@/state/tabs';
import { callsTo, expectCallCount, faroError, mockInvoke } from '@/test/ipc';

// QueryTab's own logic (run/cancel guards, selection precedence, store
// updates) is what these tests target — CodeMirror's own behaviour is
// covered in Editor.test.tsx, so it is replaced with a plain textarea that
// exposes the same handle shape.
const editorMock = vi.hoisted(() => ({ selection: null as string | null }));

vi.mock('@/features/editor/Editor', () => ({
  Editor: ({
    value,
    onChange,
    handle,
  }: {
    value: string;
    onChange: (v: string) => void;
    handle?: React.RefObject<{
      selection: () => string | null;
      cursor: () => number;
      replaceAll: (text: string) => void;
    } | null>;
  }) => {
    if (handle) {
      handle.current = {
        selection: () => editorMock.selection,
        cursor: () => 0,
        replaceAll: vi.fn(),
      };
    }
    return (
      <textarea aria-label="sql editor" value={value} onChange={(e) => onChange(e.target.value)} />
    );
  },
}));

function connection(overrides: Partial<ConnectionStatus> = {}): ConnectionStatus {
  return {
    id: 'c1',
    name: 'Local Postgres',
    engine: 'postgres',
    host: 'localhost',
    port: 5432,
    username: 'postgres',
    database: 'postgres',
    filePath: null,
    sslMode: 'prefer',
    color: null,
    readOnly: false,
    connected: true,
    hasPassword: true,
    ...overrides,
  };
}

function runResult(sql = 'select 1'): RunResult {
  return {
    statements: [
      {
        sql,
        outcome: { type: 'rows', columns: [], rows: [], truncated: false, elapsedMs: 1 },
        error: null,
      },
    ],
    totalElapsedMs: 1,
  };
}

function currentTab(): Tab {
  const t = useTabs.getState().activeTab();
  if (!t) throw new Error('no active tab');
  return t;
}

// QueryTab is a plain prop-driven component — in the real app, App.tsx
// subscribes to the store and re-renders it with a fresh `tab` on every
// change. A static snapshot prop would never see `update()`'s effects, so
// this harness reproduces that same subscription for the test.
function Harness({ id }: { id: string }) {
  const tab = useTabs((s) => s.tabs.find((t) => t.id === id));
  if (!tab) return null;
  return <QueryTab tab={tab} />;
}

function renderQueryTab(sql = 'select 1') {
  useConnections.setState({ items: [connection()] });
  const id = useTabs.getState().openQueryTab('c1', sql, 'Query 1');
  const view = render(<Harness id={id} />);
  return { ...view, id };
}

beforeEach(() => {
  vi.clearAllMocks();
  useTabs.setState({ tabs: [], activeId: null });
  useConnections.setState({ items: [] });
  useSchemaCache.setState({ byConnection: {}, loading: new Set() });
  editorMock.selection = null;
  mockInvoke({});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('QueryTab', () => {
  it('disables Run without a connection or with blank SQL', () => {
    useConnections.setState({ items: [] });
    const id = useTabs.getState().openQueryTab(null, '', 'Query 1');
    render(<Harness id={id} />);
    expect(screen.getByRole('button', { name: /Run/ })).toBeDisabled();
  });

  it('runs the tab’s SQL against its connection and stores the results', async () => {
    mockInvoke({ run_query: () => runResult() });
    renderQueryTab('select * from users');

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));

    // The store flips to running immediately, synchronously with the click.
    expect(currentTab().running).toBe(true);

    expect(await screen.findByRole('button', { name: /Cancel/ })).toBeInTheDocument();
    expectCallCount('run_query', 1);
    expect(callsTo('run_query')[0]).toMatchObject({
      connectionId: 'c1',
      sqlText: 'select * from users',
    });
    expect(currentTab().running).toBe(false);
    expect(currentTab().results).toHaveLength(1);
  });

  it('runs only the selected text when the editor reports a selection', async () => {
    mockInvoke({ run_query: () => runResult() });
    renderQueryTab('select * from users');
    editorMock.selection = 'select * from orders';

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));
    await screen.findByRole('button', { name: /Run/ }); // wait for it to finish

    expect(callsTo('run_query')[0]).toMatchObject({ sqlText: 'select * from orders' });
  });

  it('ignores a second Run while one is already in flight', async () => {
    let resolveRun: (r: RunResult) => void = () => {};
    mockInvoke({
      run_query: () =>
        new Promise<RunResult>((resolve) => {
          resolveRun = resolve;
        }),
    });
    renderQueryTab();

    const runButton = screen.getByRole('button', { name: /Run/ });
    fireEvent.click(runButton);
    // Two ⌘↵ presses landing in the same tick is exactly the race this guards.
    fireEvent.keyDown(window, { key: 'Enter', metaKey: true });
    fireEvent.keyDown(window, { key: 'Enter', metaKey: true });

    resolveRun(runResult());
    await screen.findByRole('button', { name: /Run/ });

    expectCallCount('run_query', 1);
  });

  it('records a failure without touching the previous results', async () => {
    mockInvoke({ run_query: faroError('database', 'relation "users" does not exist') });
    renderQueryTab();

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));
    await screen.findByRole('button', { name: /Run/ });

    expect(currentTab().running).toBe(false);
    expect(currentTab().error).toBe('relation "users" does not exist');
    expect(currentTab().results).toHaveLength(0);
  });

  it('cancels the in-flight query through the backend', async () => {
    mockInvoke({
      run_query: () => new Promise<RunResult>(() => {}), // never resolves
      cancel_query: () => true,
    });
    renderQueryTab();

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));
    const cancelButton = await screen.findByRole('button', { name: /Cancel/ });
    fireEvent.click(cancelButton);

    expectCallCount('cancel_query', 1);
    expect(callsTo('cancel_query')[0]).toMatchObject({ connectionId: 'c1' });
  });
});
