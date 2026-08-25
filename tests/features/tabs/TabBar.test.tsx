import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { TabBar } from '@/features/tabs/TabBar';
import type { ConnectionStatus } from '@/ipc/types';
import { useConnections } from '@/state/connections';
import { useTabs } from '@/state/tabs';

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

function resetStores() {
  useTabs.setState({ tabs: [], activeId: null });
  useConnections.setState({ items: [] });
}

beforeEach(resetStores);
afterEach(() => {
  resetStores();
  vi.restoreAllMocks();
});

describe('TabBar', () => {
  it('renders every open tab and highlights the active one', async () => {
    useConnections.setState({ items: [connection()] });
    const a = useTabs.getState().openQueryTab('c1', 'select 1', 'Query 1');
    useTabs.getState().openQueryTab('c1', 'select 2', 'Query 2');
    await useTabs.getState().setActive(a);

    render(<TabBar />);
    expect(screen.getByText('Query 1')).toBeInTheDocument();
    expect(screen.getByText('Query 2')).toBeInTheDocument();
    expect(useTabs.getState().activeId).toBe(a);
  });

  it('activates a tab on click', async () => {
    useConnections.setState({ items: [connection()] });
    useTabs.getState().openQueryTab('c1', 'select 1', 'Query 1');
    const b = useTabs.getState().openQueryTab('c1', 'select 2', 'Query 2');
    // openQueryTab focuses the newly opened tab, so start from the first one.
    await useTabs.getState().setActive(useTabs.getState().tabs[0]!.id);

    render(<TabBar />);
    fireEvent.click(screen.getByText('Query 2'));
    await waitFor(() => expect(useTabs.getState().activeId).toBe(b));
  });

  it('closes a tab via its close button', async () => {
    useConnections.setState({ items: [connection()] });
    useTabs.getState().openQueryTab('c1', 'select 1', 'Query 1');

    render(<TabBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Close Query 1' }));
    await waitFor(() => expect(useTabs.getState().tabs).toHaveLength(0));
  });

  it('closes a tab on a middle click, without activating it first', async () => {
    useConnections.setState({ items: [connection()] });
    const a = useTabs.getState().openQueryTab('c1', 'select 1', 'Query 1');
    useTabs.getState().openQueryTab('c1', 'select 2', 'Query 2');
    await useTabs.getState().setActive(a);

    render(<TabBar />);
    fireEvent(
      screen.getByText('Query 2'),
      new MouseEvent('auxclick', { button: 1, bubbles: true }),
    );

    await waitFor(() => expect(useTabs.getState().tabs.map((t) => t.id)).toEqual([a]));
    expect(useTabs.getState().activeId).toBe(a);
  });

  it('disables the new-tab button when nothing is connected', () => {
    render(<TabBar />);
    expect(screen.getByTitle('Connect to a database first')).toBeDisabled();
  });

  it('opens a new query tab against the active tab’s connection', () => {
    useConnections.setState({ items: [connection({ id: 'c1' }), connection({ id: 'c2' })] });
    useTabs.getState().openQueryTab('c2', '', 'Query 1');

    render(<TabBar />);
    fireEvent.click(screen.getByTitle('New query tab'));

    const tabs = useTabs.getState().tabs;
    expect(tabs).toHaveLength(2);
    expect(tabs[1]!.connectionId).toBe('c2');
  });
});
