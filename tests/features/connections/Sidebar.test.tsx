import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ConfirmHost } from '@/components/ConfirmDialog';
import { Sidebar } from '@/features/connections/Sidebar';
import type { ConnectionStatus } from '@/ipc/types';
import { useTabs } from '@/state/tabs';
import { expectCallCount, mockInvoke } from '@/test/ipc';

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

beforeEach(() => {
  useTabs.setState({ tabs: [], activeId: null });
  mockInvoke({
    list_connection_status: () => [connection()],
    keychain_available: () => true,
    list_schemas: () => [{ name: 'public', isSystem: false }],
    list_tables: () => [{ schema: 'public', name: 'users', kind: 'table', estimatedRows: 10 }],
    // The sidebar also renders the saved-queries/history panel below the
    // connection tree; it needs real arrays back, not the mock's default
    // `undefined`, or it throws on `.length` during its own mount effect.
    list_saved_queries: () => [],
    list_history: () => [],
    // The (closed) connection dialog is always mounted alongside the tree
    // and fetches the engine list on its own mount effect.
    list_engines: () => [],
  });
});

describe('Sidebar schema tree', () => {
  it('opens a table on click', async () => {
    render(<Sidebar />);
    fireEvent.click(await screen.findByText('Local Postgres'));

    fireEvent.click(await screen.findByText('users'));

    const tabs = useTabs.getState().tabs;
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({ kind: 'table', title: 'users' });
  });

  it('table rows are keyboard-reachable, like every other row in the tree', async () => {
    // Regression test: table rows were plain onClick <div>s with no role,
    // tabIndex or key handler, unlike the schema-toggle and connection rows
    // right above them in the same tree.
    render(<Sidebar />);
    fireEvent.click(await screen.findByText('Local Postgres'));

    const label = await screen.findByText('users');
    const row = label.closest('[role="button"]');
    expect(row).not.toBeNull();
    expect(row).toHaveAttribute('tabindex', '0');

    fireEvent.keyDown(row!, { key: 'Enter' });
    expect(useTabs.getState().tabs).toHaveLength(1);
  });
});

describe('Sidebar disconnect', () => {
  // Call counts below are per test; the shared invoke mock otherwise keeps
  // the previous test's calls.
  beforeEach(() => {
    vi.clearAllMocks();
  });

  function renderWithConfirm() {
    return render(
      <>
        <Sidebar />
        <ConfirmHost />
      </>,
    );
  }

  it('disconnects straight away when nothing is at stake', async () => {
    mockInvoke({
      list_connection_status: () => [connection()],
      keychain_available: () => true,
      list_saved_queries: () => [],
      list_history: () => [],
      list_engines: () => [],
      disconnect: () => undefined,
    });
    renderWithConfirm();

    fireEvent.click(await screen.findByRole('button', { name: 'Disconnect Local Postgres' }));

    await waitFor(() => expectCallCount('disconnect', 1));
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
  });

  it('asks first when a tab on the connection is running a query', async () => {
    // Disconnecting cancels it; that used to happen without a word.
    mockInvoke({
      list_connection_status: () => [connection()],
      keychain_available: () => true,
      list_saved_queries: () => [],
      list_history: () => [],
      list_engines: () => [],
      disconnect: () => undefined,
    });
    const id = useTabs.getState().openQueryTab('c1', 'select pg_sleep(60)', 'Query 1');
    useTabs.getState().update(id, { running: true });
    renderWithConfirm();

    fireEvent.click(await screen.findByRole('button', { name: 'Disconnect Local Postgres' }));

    expect(await screen.findByText(/stops the query running on it/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    await waitFor(() =>
      expect(screen.queryByText(/stops the query running on it/)).not.toBeInTheDocument(),
    );
    expectCallCount('disconnect', 0);
  });

  it('uses a different icon for disconnecting than for deleting', async () => {
    // One trash can used to mean both.
    renderWithConfirm();
    const disconnect = await screen.findByRole('button', { name: 'Disconnect Local Postgres' });
    expect(disconnect).toHaveAttribute('title', 'Disconnect');
    expect(screen.queryByRole('button', { name: 'Delete Local Postgres' })).not.toBeInTheDocument();
  });
});
