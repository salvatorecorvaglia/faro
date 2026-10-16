import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ConfirmHost } from '@/components/ConfirmDialog';
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
const editorMock = vi.hoisted(() => ({
  selection: null as string | null,
  /** Offset the mock reports through `onCursorChange` once mounted. */
  cursor: 0,
}));

vi.mock('@/features/editor/Editor', () => ({
  Editor: ({
    value,
    onChange,
    onCursorChange,
    handle,
  }: {
    value: string;
    onChange: (v: string) => void;
    onCursorChange?: (offset: number) => void;
    handle?: React.RefObject<{
      selection: () => string | null;
      replaceAll: (text: string) => void;
      cursorOffset: () => number;
    } | null>;
  }) => {
    if (handle) {
      handle.current = {
        selection: () => editorMock.selection,
        replaceAll: vi.fn(),
        // Read live, like the real editor's handle: a test can move the
        // cursor without a render and the next read sees it.
        cursorOffset: () => editorMock.cursor,
      };
    }
    // The real editor reports the cursor from a CodeMirror update listener;
    // firing it once on render is enough to drive the statement hint.
    onCursorChange?.(editorMock.cursor);
    return (
      <textarea aria-label="sql editor" value={value} onChange={(e) => onChange(e.target.value)} />
    );
  },
}));

/**
 * Answer the commands QueryTab issues on mount, plus whatever a test needs.
 *
 * `schema_snapshot` feeds editor autocompletion and fires for every tab with a
 * connection, so it is incidental to all of these tests — but the mock refuses
 * undeclared commands, so it still has to be named somewhere. Once, here.
 */
function mockQueryTab(handlers: Parameters<typeof mockInvoke>[0] = {}) {
  return mockInvoke({ schema_snapshot: () => [], ...handlers });
}

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
  editorMock.cursor = 0;
  mockQueryTab({});
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
    mockQueryTab({ run_query: () => runResult() });
    renderQueryTab('select * from users');

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));

    // Not synchronous with the click any more: the script is summarized first,
    // so a run that writes can be confirmed before anything is sent.
    await waitFor(() => expectCallCount('run_query', 1));
    await waitFor(() => expect(currentTab().running).toBe(false));
    expect(callsTo('run_query')[0]).toMatchObject({
      connectionId: 'c1',
      sqlText: 'select * from users',
    });
    expect(currentTab().running).toBe(false);
    expect(currentTab().results).toHaveLength(1);
    // Exports name these results by the id they came back under; the backend
    // writes its own copy rather than the UI sending the rows back.
    expect(currentTab().resultQueryId).toBe(callsTo('run_query')[0]!.queryId);
  });

  it('opens the save dialog on Cmd+S', async () => {
    // The shortcut used to live in App with its own SaveQueryDialog, so two
    // were mounted and the key and the toolbar button opened different ones.
    mockQueryTab({ run_query: () => runResult(), list_saved_queries: () => [] });
    renderQueryTab('select 1');

    fireEvent.keyDown(document.body, { key: 's', metaKey: true });

    expect(await screen.findByRole('heading', { name: /Save query/i })).toBeInTheDocument();
  });

  it('ignores Cmd+S when the tab has no SQL to save', () => {
    mockQueryTab({ list_saved_queries: () => [] });
    useConnections.setState({ items: [connection()] });
    const id = useTabs.getState().openQueryTab('c1', '   ', 'Query 1');
    render(<Harness id={id} />);

    fireEvent.keyDown(document.body, { key: 's', metaKey: true });

    expect(screen.queryByRole('heading', { name: /Save query/i })).not.toBeInTheDocument();
  });

  it('sends the tab’s row limit with the run', async () => {
    // Every run was pinned to the backend's default page and the UI offered no
    // way to raise it, so a truncated result could only be widened by editing
    // the query itself.
    mockQueryTab({ run_query: () => runResult() });
    renderQueryTab('select * from users');

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));
    await screen.findByRole('button', { name: /Run/ });

    expect(callsTo('run_query')[0]).toMatchObject({ limit: 1000 });
  });

  it('runs with a raised row limit once the user picks one', async () => {
    mockQueryTab({ run_query: () => runResult() });
    renderQueryTab('select * from users');

    fireEvent.change(screen.getByTitle('Most rows a run may return'), {
      target: { value: '50000' },
    });
    expect(currentTab().rowLimit).toBe(50000);

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));
    await screen.findByRole('button', { name: /Run/ });

    expect(callsTo('run_query')[0]).toMatchObject({ limit: 50000 });
  });

  it('runs only the selected text when the editor reports a selection', async () => {
    mockQueryTab({ run_query: () => runResult() });
    renderQueryTab('select * from users');
    editorMock.selection = 'select * from orders';

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));
    await screen.findByRole('button', { name: /Run/ }); // wait for it to finish

    expect(callsTo('run_query')[0]).toMatchObject({ sqlText: 'select * from orders' });
  });

  it('offers the statement under the cursor, and runs only that one', async () => {
    // `statement_at_cursor` resolves this on the backend, because the offset is
    // a UTF-16 CodeMirror position and the splitter it must agree with lives
    // there too.
    mockQueryTab({
      statement_at_cursor: () => 'select * from orders',
      run_query: () => runResult(),
    });
    renderQueryTab('select * from users;\nselect * from orders');
    editorMock.cursor = 25;

    const hint = await screen.findByRole('button', { name: /select \* from orders/ });

    fireEvent.click(hint);
    // The statement is re-resolved before the run starts, so the Run button is
    // still on screen for a moment — wait for the run itself.
    await waitFor(() => expectCallCount('run_query', 1));

    expect(callsTo('run_query')[0]).toMatchObject({ sqlText: 'select * from orders' });
  });

  it('runs the statement under the cursor as it is now, not as the hint last saw it', async () => {
    // The hint is debounced, so right after the cursor moves it still names the
    // previous statement. Running that one executed a statement the cursor had
    // already left.
    const sql = 'select * from users;\ndelete from orders';
    mockQueryTab({
      statement_at_cursor: ({ offset }) =>
        (offset as number) > 20 ? 'delete from orders' : 'select * from users',
      run_query: () => runResult(),
    });
    renderQueryTab(sql);

    const hint = await screen.findByRole('button', { name: /select \* from users/ });

    // The cursor moves, but no render — and so no new hint — has happened yet.
    editorMock.cursor = 25;
    fireEvent.click(hint);
    await waitFor(() => expectCallCount('run_query', 1));

    const lookups = callsTo('statement_at_cursor');
    expect(lookups[lookups.length - 1]).toMatchObject({ sqlText: sql, offset: 25 });
    expect(callsTo('run_query')[0]).toMatchObject({ sqlText: 'delete from orders' });
  });

  it('does not offer a statement hint for an empty editor', async () => {
    mockQueryTab({ statement_at_cursor: () => 'select 1' });
    renderQueryTab('');

    // No round trip at all: there is nothing for the cursor to sit in.
    await Promise.resolve();
    expectCallCount('statement_at_cursor', 0);
  });

  it('ignores a second Run while one is already in flight', async () => {
    let resolveRun: (r: RunResult) => void = () => {};
    mockQueryTab({
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
    mockQueryTab({ run_query: faroError('database', 'relation "users" does not exist') });
    renderQueryTab();

    fireEvent.click(screen.getByRole('button', { name: /Run/ }));
    await screen.findByRole('button', { name: /Run/ });

    expect(currentTab().running).toBe(false);
    expect(currentTab().error).toBe('relation "users" does not exist');
    expect(currentTab().results).toHaveLength(0);
  });

  describe('running a whole script', () => {
    function renderWithConfirm(sql: string) {
      useConnections.setState({ items: [connection()] });
      const id = useTabs.getState().openQueryTab('c1', sql, 'Query 1');
      render(
        <>
          <Harness id={id} />
          <ConfirmHost />
        </>,
      );
    }

    it('asks before running several statements that change data, and runs nothing if declined', async () => {
      // ⌘↵ runs the whole editor. Pressed out of habit on one SELECT, it also
      // ran the DELETE below it.
      mockQueryTab({
        script_summary: () => ({ statements: 2, writes: 1 }),
        run_query: () => runResult(),
      });
      renderWithConfirm('select * from users;\ndelete from orders');

      fireEvent.click(screen.getByRole('button', { name: /Run/ }));

      expect(await screen.findByText(/1 of them changes data or schema/)).toBeInTheDocument();
      fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));

      await waitFor(() => expect(screen.queryByText(/of them change/)).not.toBeInTheDocument());
      expectCallCount('run_query', 0);
    });

    it('runs the script once the user confirms', async () => {
      mockQueryTab({
        script_summary: () => ({ statements: 3, writes: 2 }),
        run_query: () => runResult(),
      });
      renderWithConfirm('select 1; delete from a; delete from b');

      fireEvent.click(screen.getByRole('button', { name: /Run/ }));
      fireEvent.click(await screen.findByRole('button', { name: 'Run all' }));

      await waitFor(() => expectCallCount('run_query', 1));
      expect(callsTo('run_query')[0]).toMatchObject({
        sqlText: 'select 1; delete from a; delete from b',
      });
    });

    it('does not ask for a single statement, even a write', async () => {
      // Running one DELETE on purpose is the ordinary case, not a surprise.
      mockQueryTab({
        script_summary: () => ({ statements: 1, writes: 1 }),
        run_query: () => runResult(),
      });
      renderWithConfirm('delete from orders where id = 1');

      fireEvent.click(screen.getByRole('button', { name: /Run/ }));

      await waitFor(() => expectCallCount('run_query', 1));
      expect(screen.queryByRole('button', { name: 'Run all' })).not.toBeInTheDocument();
    });

    it('does not ask for a script that only reads', async () => {
      mockQueryTab({
        script_summary: () => ({ statements: 3, writes: 0 }),
        run_query: () => runResult(),
      });
      renderWithConfirm('select 1; select 2; select 3');

      fireEvent.click(screen.getByRole('button', { name: /Run/ }));

      await waitFor(() => expectCallCount('run_query', 1));
    });
  });

  it('cancels the in-flight query through the backend', async () => {
    mockQueryTab({
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
