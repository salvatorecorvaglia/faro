import { save } from '@tauri-apps/plugin-dialog';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ExportDialog } from '@/features/transfer/ExportDialog';
import type { ResultSet } from '@/ipc/types';
import { callsTo, expectCallCount, faroError, mockInvoke } from '@/test/ipc';

/**
 * `ImportDialog` had a test; its sibling had none, and none of
 * `suggested_export_name`, `export_table` or the result export was mocked
 * anywhere in the repo.
 *
 * The distinction this dialog exists to offer — the fetched page versus the
 * whole table — is the part worth pinning: getting it wrong writes a file that
 * looks complete and quietly stops at one page.
 */

const result: ResultSet = {
  columns: [{ name: 'id', typeName: 'int4' }],
  rows: [[{ kind: 'int', value: 1 }]],
  truncated: true,
  elapsedMs: 1,
};

const table = { schema: 'public', name: 'users' };

function renderExport(props: Partial<React.ComponentProps<typeof ExportDialog>> = {}) {
  return render(
    <ExportDialog
      open
      onClose={() => {}}
      result={result}
      source={{ queryId: 'b-7', statement: 0 }}
      connectionId="c1"
      table={table}
      defaultName="users"
      {...props}
    />,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(save).mockResolvedValue('/tmp/users.csv');
});

describe('ExportDialog', () => {
  it('re-reads the whole table by default when one is available', async () => {
    mockInvoke({
      suggested_export_name: () => 'users.csv',
      export_table: () => ({ rows: 50000, path: '/tmp/users.csv' }),
    });
    renderExport();

    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    expect(await screen.findByText(/Wrote 50,000 rows/)).toBeInTheDocument();
    // The grid holds one page; exporting that would silently truncate.
    expectCallCount('export_cached', 0);
    const call = callsTo('export_table')[0];
    expect(call).toMatchObject({ connectionId: 'c1', table, path: '/tmp/users.csv' });
    expect(call?.queryId).toEqual(expect.stringMatching(/^exp-\d+$/));
  });

  it('exports just the fetched page when the user picks that scope', async () => {
    mockInvoke({
      suggested_export_name: () => 'users.csv',
      export_cached: () => ({ rows: 1, path: '/tmp/users.csv' }),
    });
    renderExport();

    await userEvent.click(screen.getByRole('radio', { name: /Just this page/ }));
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    await screen.findByText(/Wrote 1 rows/);
    expectCallCount('export_table', 0);
    // Named by the query id the page came back under; the rows themselves are
    // not sent back — the backend writes its own, faithful copy.
    const call = callsTo('export_cached')[0];
    expect(call).toMatchObject({ queryId: 'b-7', statement: 0, path: '/tmp/users.csv' });
    expect(call).not.toHaveProperty('result');
  });

  it('exports the result directly when there is no table behind it', async () => {
    // A query tab's result set has no table to re-read.
    mockInvoke({
      suggested_export_name: () => 'query.json',
      export_cached: () => ({ rows: 1, path: '/tmp/query.json' }),
    });
    renderExport({
      table: null,
      defaultName: 'query',
      source: { queryId: 'q-3', statement: 2 },
    });

    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    await screen.findByText(/Wrote 1 rows/);
    expectCallCount('export_table', 0);
    // The statement on screen, of the run on screen.
    expect(callsTo('export_cached')[0]).toMatchObject({ queryId: 'q-3', statement: 2 });
  });

  it('says to run the query again when there is no held result to export', async () => {
    mockInvoke({ suggested_export_name: () => 'query.csv' });
    renderExport({ table: null, defaultName: 'query', source: null });

    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    expect(await screen.findByText(/Run the query again/)).toBeInTheDocument();
    expectCallCount('export_cached', 0);
  });

  it('asks the backend for a filename in the chosen format', async () => {
    mockInvoke({
      suggested_export_name: () => 'users.json',
      export_table: () => ({ rows: 1, path: '/tmp/users.json' }),
    });
    renderExport();

    await userEvent.click(screen.getByRole('button', { name: 'JSON' }));
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    await screen.findByText(/Wrote 1 rows/);
    expect(callsTo('suggested_export_name')[0]).toMatchObject({ base: 'users', format: 'json' });
    expect(callsTo('export_table')[0]?.options).toMatchObject({ format: 'json' });
  });

  it('carries the header and formula-sanitizing choices through', async () => {
    mockInvoke({
      suggested_export_name: () => 'users.csv',
      export_table: () => ({ rows: 1, path: '/tmp/users.csv' }),
    });
    renderExport();

    await userEvent.click(screen.getByRole('checkbox', { name: /Include a header row/ }));
    await userEvent.click(screen.getByRole('checkbox', { name: /Escape spreadsheet formulas/ }));
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    await screen.findByText(/Wrote 1 rows/);
    expect(callsTo('export_table')[0]?.options).toMatchObject({
      includeHeader: false,
      sanitizeFormulas: false,
    });
  });

  it('writes nothing when the save dialog is dismissed', async () => {
    mockInvoke({
      suggested_export_name: () => 'users.csv',
      export_table: () => ({ rows: 1, path: '' }),
    });
    vi.mocked(save).mockResolvedValue(null);
    renderExport();

    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    expectCallCount('export_table', 0);
    expect(screen.queryByText(/Wrote/)).not.toBeInTheDocument();
  });

  it('surfaces a failed export', async () => {
    mockInvoke({
      suggested_export_name: () => 'users.csv',
      export_table: faroError('io', 'no space left on device'),
    });
    renderExport();

    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    expect(await screen.findByText(/no space left on device/)).toBeInTheDocument();
  });
});
