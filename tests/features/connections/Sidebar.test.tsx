import { fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';

import { Sidebar } from '@/features/connections/Sidebar';
import type { ConnectionStatus } from '@/ipc/types';
import { useTabs } from '@/state/tabs';
import { mockInvoke } from '@/test/ipc';

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
