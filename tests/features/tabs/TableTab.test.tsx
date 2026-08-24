import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { TableTab } from '@/features/tabs/TableTab';
import type { ConnectionStatus, ResultSet, TableDetail, Value } from '@/ipc/types';
import { useConnections } from '@/state/connections';
import { type Tab, useTabs } from '@/state/tabs';
import { callsTo, expectCallCount, mockInvoke } from '@/test/ipc';

const int = (n: number): Value => ({ kind: 'int', value: n });
const text = (s: string): Value => ({ kind: 'text', value: s });

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

function detail(overrides: Partial<TableDetail> = {}): TableDetail {
  return {
    table: { schema: 'public', name: 'users' },
    kind: 'table',
    columns: [
      {
        name: 'id',
        typeName: 'int4',
        nullable: false,
        default: null,
        isPrimaryKey: true,
        ordinal: 0,
      },
      {
        name: 'name',
        typeName: 'text',
        nullable: true,
        default: null,
        isPrimaryKey: false,
        ordinal: 1,
      },
    ],
    primaryKey: ['id'],
    foreignKeys: [],
    indexes: [],
    ...overrides,
  };
}

function browseResult(rows: Value[][], overrides: Partial<ResultSet> = {}): ResultSet {
  return {
    columns: [
      { name: 'id', typeName: 'int4' },
      { name: 'name', typeName: 'text' },
    ],
    rows,
    truncated: false,
    elapsedMs: 1,
    ...overrides,
  };
}

// App.tsx re-renders TableTab with a fresh `tab` prop on every store change;
// a static snapshot would never see load()/update()'s effects.
function Harness({ id }: { id: string }) {
  const tab = useTabs((s) => s.tabs.find((t) => t.id === id));
  if (!tab) return null;
  return <TableTab tab={tab} />;
}

function currentTab(): Tab {
  const t = useTabs.getState().activeTab();
  if (!t) throw new Error('no active tab');
  return t;
}

function renderTableTab(connectionOverrides: Partial<ConnectionStatus> = {}) {
  useConnections.setState({ items: [connection(connectionOverrides)] });
  const id = useTabs.getState().openTableTab('c1', { schema: 'public', name: 'users' });
  const view = render(<Harness id={id} />);
  return { ...view, id };
}

beforeEach(() => {
  vi.clearAllMocks();
  useTabs.setState({ tabs: [], activeId: null });
  useConnections.setState({ items: [] });
  mockInvoke({});
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('TableTab', () => {
  it('loads the table on mount and renders its rows', async () => {
    mockInvoke({
      describe_table: () => detail(),
      browse_table: () => browseResult([[int(1), text('Alice')]]),
    });
    renderTableTab();

    expect(await screen.findByText('Alice')).toBeInTheDocument();
    expect(screen.getByText(/2 columns/)).toBeInTheDocument();
    expect(screen.getByText(/PK: id/)).toBeInTheDocument();
    expect(callsTo('browse_table')[0]).toMatchObject({
      connectionId: 'c1',
      table: { schema: 'public', name: 'users' },
    });
  });

  describe('pagination', () => {
    it('disables Previous on the first page and enables Next when more rows exist', async () => {
      mockInvoke({
        describe_table: () => detail(),
        browse_table: () => browseResult([[int(1), text('Alice')]], { truncated: true }),
      });
      renderTableTab();
      await screen.findByText('Alice');

      expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled();
      expect(screen.getByRole('button', { name: 'Next' })).toBeEnabled();
    });

    it('requests the next page at the right offset', async () => {
      mockInvoke({
        describe_table: () => detail(),
        browse_table: () => browseResult([[int(1), text('Alice')]], { truncated: true }),
      });
      renderTableTab();
      await screen.findByText('Alice');

      fireEvent.click(screen.getByRole('button', { name: 'Next' }));
      await waitFor(() => expectCallCount('browse_table', 2));
      expect(callsTo('browse_table')[1]).toMatchObject({ options: { offset: 1000 } });
    });
  });

  describe('editability', () => {
    it('offers Add row / Import for a table with a primary key', async () => {
      mockInvoke({
        describe_table: () => detail(),
        browse_table: () => browseResult([[int(1), text('Alice')]]),
      });
      renderTableTab();
      await screen.findByText('Alice');

      expect(screen.getByRole('button', { name: 'Row' })).toBeInTheDocument();
      expect(screen.getByRole('button', { name: 'Import' })).toBeInTheDocument();
    });

    it('explains why a table with no primary key is read-only, and hides edit affordances', async () => {
      mockInvoke({
        describe_table: () => detail({ primaryKey: [] }),
        browse_table: () => browseResult([[int(1), text('Alice')]]),
      });
      renderTableTab();
      await screen.findByText('Alice');

      expect(screen.getByText(/no primary key/)).toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Row' })).not.toBeInTheDocument();
      expect(screen.queryByTitle('Import a file into this table')).not.toBeInTheDocument();
    });

    it('explains a read-only connection even when the table has a primary key', async () => {
      mockInvoke({
        describe_table: () => detail(),
        browse_table: () => browseResult([[int(1), text('Alice')]]),
      });
      renderTableTab({ readOnly: true });
      await screen.findByText('Alice');

      expect(screen.getByText(/connection is open read-only/)).toBeInTheDocument();
    });
  });

  it('discards a stale response that resolves after a newer request for the same table', async () => {
    // A sort click right after a filter edit starts a second load before the
    // first one returns; whichever resolves last must not win over whichever
    // was started last. Sort clicks (unlike Refresh) aren't disabled while a
    // request is in flight, which is what makes them able to race at all.
    let resolveSecond: (r: ResultSet) => void = () => {};
    let callCount = 0;
    mockInvoke({
      describe_table: () => detail(),
      browse_table: () => {
        callCount += 1;
        if (callCount === 1) return browseResult([[int(1), text('Alice')]]);
        if (callCount === 2) {
          return new Promise<ResultSet>((resolve) => {
            resolveSecond = resolve;
          });
        }
        return browseResult([[int(2), text('Bob')]]);
      },
    });
    renderTableTab();
    await screen.findByText('Alice');

    // Second load: sorting by id. Left pending.
    fireEvent.click(screen.getByText('id'));
    await waitFor(() => expectCallCount('browse_table', 2));

    // Third load: sorting again (descending). Resolves immediately, landing
    // before the second one does.
    fireEvent.click(screen.getByText('id'));
    await waitFor(() => expectCallCount('browse_table', 3));
    expect(await screen.findByText('Bob')).toBeInTheDocument();

    // The second request finally resolves. Its result must be dropped, since
    // a third request had already started (and finished) by then.
    resolveSecond(browseResult([[int(3), text('Stale Carol')]]));
    await new Promise((r) => setTimeout(r, 0));

    expect(screen.queryByText('Stale Carol')).not.toBeInTheDocument();
    expect(screen.getByText('Bob')).toBeInTheDocument();
  });

  it('marks the tab dirty in the store after a cell edit, and clean again after discarding', async () => {
    mockInvoke({
      describe_table: () => detail(),
      browse_table: () => browseResult([[int(1), text('Alice')]]),
    });
    renderTableTab();
    await screen.findByText('Alice');
    expect(currentTab().dirty).toBe(false);

    fireEvent.doubleClick(screen.getByText('Alice'));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'Alicia' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(currentTab().dirty).toBe(true));
    expect(screen.getByText(/1 unsaved change/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Discard' }));
    await waitFor(() => expect(currentTab().dirty).toBe(false));
  });

  it('confirms before a refresh discards unsaved edits', async () => {
    mockInvoke({
      describe_table: () => detail(),
      browse_table: () => browseResult([[int(1), text('Alice')]]),
    });
    renderTableTab();
    await screen.findByText('Alice');

    fireEvent.doubleClick(screen.getByText('Alice'));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'Alicia' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(currentTab().dirty).toBe(true));

    const confirmSpy = vi.fn(() => false);
    vi.stubGlobal('confirm', confirmSpy);

    fireEvent.click(screen.getByTitle('Refresh'));

    expect(confirmSpy).toHaveBeenCalledOnce();
    // Declined: no second load, and the edit is still there.
    expectCallCount('browse_table', 1);
    expect(currentTab().dirty).toBe(true);

    vi.unstubAllGlobals();
  });
});
