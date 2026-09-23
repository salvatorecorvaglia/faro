import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import App from '@/App';
import type { ConnectionStatus } from '@/ipc/types';
import { useConfirmStore } from '@/state/confirm';
import { useConnections } from '@/state/connections';
import { useTabs } from '@/state/tabs';
import { mockInvoke } from '@/test/ipc';

// App's own job is the shell: which tab is mounted, and the global shortcuts.
// The editor is CodeMirror, covered in Editor.test.tsx, and mounting it for
// every shortcut assertion is slow and irrelevant here.
vi.mock('@/features/editor/Editor', () => ({
  Editor: ({ value }: { value: string }) => (
    <textarea aria-label="sql editor" defaultValue={value} />
  ),
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

/** Press a key on document.body, i.e. not from inside a field. */
const press = (key: string, init: KeyboardEventInit = {}) =>
  fireEvent.keyDown(document.body, { key, ...init });

beforeEach(() => {
  useTabs.setState({ tabs: [], activeId: null });
  useConfirmStore.setState({ request: null });
  useConnections.setState({
    items: [connection()],
    loading: false,
    connecting: new Set(),
    error: null,
    keychainOk: true,
  });
  mockInvoke({
    list_connection_status: () => [connection()],
    keychain_available: () => true,
    list_schemas: () => [{ name: 'public', isSystem: false }],
    list_tables: () => [],
    list_saved_queries: () => [],
    list_history: () => [],
    list_engines: () => [],
    schema_snapshot: () => [],
  });
});

describe('App shell', () => {
  it('invites the user to connect when nothing is open', () => {
    useConnections.setState({ items: [connection({ connected: false })] });
    render(<App />);
    expect(screen.getByText('Connect to a database')).toBeInTheDocument();
  });

  it('opens a query tab with Cmd+T, inheriting the active connection', () => {
    render(<App />);
    press('t', { metaKey: true });

    const { tabs, activeId } = useTabs.getState();
    expect(tabs).toHaveLength(1);
    expect(tabs[0]!.connectionId).toBe('c1');
    expect(activeId).toBe(tabs[0]!.id);
  });

  it('closes the active tab with Cmd+W', async () => {
    render(<App />);
    press('t', { metaKey: true });
    expect(useTabs.getState().tabs).toHaveLength(1);

    press('w', { metaKey: true });
    await waitFor(() => expect(useTabs.getState().tabs).toHaveLength(0));
  });

  it('toggles the shortcut sheet with ?', () => {
    render(<App />);
    press('?');
    expect(screen.getByRole('heading', { name: 'Keyboard shortcuts' })).toBeInTheDocument();
  });

  it('does not steal ? from a field the user is typing in', () => {
    render(<App />);
    const sidebarSearch = document.createElement('input');
    document.body.appendChild(sidebarSearch);

    fireEvent.keyDown(sidebarSearch, { key: '?' });

    expect(screen.queryByRole('heading', { name: 'Keyboard shortcuts' })).not.toBeInTheDocument();
    sidebarSearch.remove();
  });

  it('asks before Cmd+W discards a tab with staged edits', async () => {
    render(<App />);
    press('t', { metaKey: true });
    const id = useTabs.getState().activeId!;
    useTabs.getState().update(id, { dirty: true });

    press('w', { metaKey: true });

    await waitFor(() => expect(useConfirmStore.getState().request).not.toBeNull());
    // Declining keeps the tab, and the work in it.
    useConfirmStore.getState().request!.resolve(false);
    useConfirmStore.setState({ request: null });

    await waitFor(() => expect(useTabs.getState().tabs).toHaveLength(1));
  });

  it('asks before Cmd+T switches away from a tab with staged edits', async () => {
    render(<App />);
    press('t', { metaKey: true });
    const first = useTabs.getState().activeId!;
    useTabs.getState().update(first, { dirty: true });

    press('t', { metaKey: true });

    await waitFor(() => expect(useConfirmStore.getState().request).not.toBeNull());
    // The second tab exists, but focus has not left the dirty one.
    expect(useTabs.getState().activeId).toBe(first);
  });
});
