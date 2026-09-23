import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ConnectionDialog } from '@/features/connections/ConnectionDialog';
import type { ConnectionConfig, EngineInfo } from '@/ipc/types';
import { useConnections } from '@/state/connections';
import { callsTo, expectCallCount, faroError, mockInvoke } from '@/test/ipc';

/**
 * The largest untested file in the app, and the one that decides what happens
 * to a stored password.
 *
 * `undefined` means "leave the saved password alone" and `''` means "clear
 * it". Those are one keystroke apart in the UI and indistinguishable in a
 * screenshot, so the distinction is worth pinning: getting it backwards either
 * wipes a working credential or silently keeps one the user meant to remove.
 */

const engines: EngineInfo[] = [
  { id: 'postgres', label: 'PostgreSQL', defaultPort: 5432, fileBased: false },
  { id: 'sqlite', label: 'SQLite', defaultPort: 0, fileBased: true },
] as unknown as EngineInfo[];

const existing: ConnectionConfig = {
  id: 'c1',
  name: 'prod',
  engine: 'postgres',
  host: 'db.example.com',
  port: 5432,
  username: 'app',
  database: 'app',
  filePath: null,
  sslMode: 'require',
  color: null,
  readOnly: false,
};

const initial = useConnections.getState();

beforeEach(() => {
  vi.clearAllMocks();
  useConnections.setState({ ...initial, items: [] });
});

function renderDialog(editing: ConnectionConfig | null = null) {
  return render(<ConnectionDialog open onClose={() => {}} editing={editing} />);
}

/** Wait for the dialog to settle after its mount-time engine fetch. */
async function ready() {
  await screen.findByRole('button', { name: /^Save$/ });
}

describe('ConnectionDialog — password handling', () => {
  it('leaves a saved password alone when editing without retyping it', async () => {
    mockInvoke({
      list_engines: () => engines,
      save_connection: () => existing,
      list_connection_status: () => [],
      keychain_available: () => true,
    });
    renderDialog(existing);
    await ready();

    await userEvent.click(screen.getByRole('button', { name: /^Save$/ }));

    await waitFor(() => expectCallCount('save_connection', 1));
    // `undefined`, not `''` — an empty string would wipe the stored password.
    expect(callsTo('save_connection')[0]).not.toHaveProperty('password', '');
    expect(callsTo('save_connection')[0]?.password).toBeUndefined();
  });

  it('sends a retyped password', async () => {
    mockInvoke({
      list_engines: () => engines,
      save_connection: () => existing,
      list_connection_status: () => [],
      keychain_available: () => true,
    });
    renderDialog(existing);
    await ready();

    // The "leave blank to keep it" wording is a hint inside the label, which
    // is exactly the affordance this test is about.
    const field = screen.getByLabelText(/Password.*Leave blank to keep the saved password/s);
    await userEvent.type(field, 'hunter2');
    await userEvent.click(screen.getByRole('button', { name: /^Save$/ }));

    await waitFor(() => expectCallCount('save_connection', 1));
    expect(callsTo('save_connection')[0]?.password).toBe('hunter2');
  });

  it('sends an empty password for a brand-new connection, not undefined', async () => {
    // A new connection has nothing stored to preserve, so "no password" has to
    // mean exactly that rather than "leave whatever is there".
    mockInvoke({
      list_engines: () => engines,
      save_connection: (args) => ({ ...(args.config as ConnectionConfig), id: 'new' }),
      list_connection_status: () => [],
      keychain_available: () => true,
    });
    renderDialog(null);
    await ready();

    await userEvent.type(screen.getByLabelText(/Display name/), 'local');
    await userEvent.click(screen.getByRole('button', { name: /^Save$/ }));

    await waitFor(() => expectCallCount('save_connection', 1));
    expect(callsTo('save_connection')[0]?.password).toBe('');
  });

  it('passes the same password to a test as to a save', async () => {
    mockInvoke({ list_engines: () => engines, test_connection: () => undefined });
    renderDialog(existing);
    await ready();

    await userEvent.type(screen.getByLabelText(/Password/), 'hunter2');
    await userEvent.click(screen.getByRole('button', { name: /Test/ }));

    await waitFor(() => expectCallCount('test_connection', 1));
    expect(callsTo('test_connection')[0]?.password).toBe('hunter2');
  });
});

describe('ConnectionDialog — failures', () => {
  it('shows why a test connection failed instead of just doing nothing', async () => {
    mockInvoke({
      list_engines: () => engines,
      test_connection: faroError('connection', 'password authentication failed for user "app"'),
    });
    renderDialog(existing);
    await ready();

    await userEvent.click(screen.getByRole('button', { name: /Test/ }));

    expect(await screen.findByText(/password authentication failed/)).toBeInTheDocument();
  });

  it('shows why a save failed and does not close over it', async () => {
    const onClose = vi.fn();
    mockInvoke({
      list_engines: () => engines,
      save_connection: faroError('store', 'settings file is read-only'),
      list_connection_status: () => [],
      keychain_available: () => true,
    });
    render(<ConnectionDialog open onClose={onClose} editing={existing} />);
    await ready();

    await userEvent.click(screen.getByRole('button', { name: /^Save$/ }));

    expect(await screen.findByText(/settings file is read-only/)).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
  });

  it('still renders when the engine list cannot be fetched', async () => {
    // The dialog is useless without engines, but crashing is worse than empty.
    mockInvoke({ list_engines: faroError('other', 'backend unavailable') });
    renderDialog(null);

    expect(await screen.findByLabelText(/Display name/)).toBeInTheDocument();
  });
});
