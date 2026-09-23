import { listen } from '@tauri-apps/api/event';
import { open as openFile, save } from '@tauri-apps/plugin-dialog';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ConfirmHost } from '@/components/ConfirmDialog';
import { BackupDialog, RestoreDialog } from '@/features/backup/BackupDialog';
import * as ipc from '@/ipc';
import type { BackupProgress, TableInfo } from '@/ipc/types';
import { callsTo, expectCallCount, faroError, mockInvoke } from '@/test/ipc';

/**
 * Backing up and restoring — the two things in Faro that write a whole
 * database — had no test that ran either of them.
 *
 * `BackupDialog.test.tsx` covers the table-selection logic and nothing else:
 * its IPC mock is empty, and `backup_database`, `restore_database` and
 * `inspect_backup` were not named in any test in the repo. Deleting the bodies
 * of `runBackup` and `runRestore` left that file green.
 */

const table = (name: string): TableInfo => ({
  schema: null,
  name,
  kind: 'table',
  estimatedRows: null,
});

/** Hand back the progress callback `useBackendProgress` registers. */
function captureProgress<T>() {
  let emit: ((payload: T) => void) | null = null;
  vi.mocked(listen).mockImplementation((async (_event: string, handler: unknown) => {
    emit = (payload: T) => (handler as (e: { payload: T }) => void)({ payload });
    return () => {};
  }) as unknown as typeof listen);
  return {
    emit: (payload: T) => {
      if (!emit) throw new Error('nothing subscribed to the progress event');
      emit(payload);
    },
  };
}

beforeEach(() => {
  // Without this `callsTo` reads the previous test's invocations too, so a
  // "nothing was called" assertion sees the call the test before it made.
  vi.clearAllMocks();
  vi.mocked(save).mockResolvedValue(null);
  vi.mocked(openFile).mockResolvedValue(null);
});

/**
 * Match only the innermost element whose text matches.
 *
 * The default text matcher also matches every ancestor, and these panels put
 * several interpolations inside one `<p>` inside a wrapper `<div>` — so a plain
 * `findByText` reports "found multiple elements" for text that appears once.
 */
function deepest(pattern: RegExp) {
  return (_content: string, element: Element | null) => {
    if (!element || !pattern.test(element.textContent ?? '')) return false;
    return !Array.from(element.children).some((child) => pattern.test(child.textContent ?? ''));
  };
}

/** The confirmation dialog's button, not the one that opened it. */
function confirmButton(name: string) {
  const all = screen.getAllByRole('button', { name });
  return all[all.length - 1] as HTMLElement;
}

describe('BackupDialog — running a backup', () => {
  function renderBackup(tables = [table('authors'), table('books')]) {
    return render(
      <BackupDialog
        open
        onClose={() => {}}
        connectionId="c1"
        connectionName="prod"
        tables={tables}
      />,
    );
  }

  it('sends the selected tables and options, then reports what it wrote', async () => {
    mockInvoke({
      backup_database: () => ({ tables: 2, rows: 1500, bytes: 4096, path: '/tmp/prod.sql' }),
    });
    vi.mocked(save).mockResolvedValue('/tmp/prod.sql');

    renderBackup();
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    expect(await screen.findByText(/Backed up 2 tables/)).toBeInTheDocument();

    const call = callsTo('backup_database')[0];
    expect(call).toMatchObject({ connectionId: 'c1', path: '/tmp/prod.sql' });
    // The dump has to name what the user actually saw ticked, not "everything".
    expect(call?.options).toMatchObject({
      tables: [
        { schema: null, name: 'authors' },
        { schema: null, name: 'books' },
      ],
      includeSchema: true,
      includeData: true,
      dropExisting: false,
    });
  });

  it('writes nothing when the save dialog is dismissed', async () => {
    mockInvoke({ backup_database: () => ({ tables: 0, rows: 0, bytes: 0, path: '' }) });
    vi.mocked(save).mockResolvedValue(null);

    renderBackup();
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    expectCallCount('backup_database', 0);
  });

  it('carries the unticked selection and the DROP option into the request', async () => {
    mockInvoke({
      backup_database: () => ({ tables: 1, rows: 1, bytes: 1, path: '/tmp/prod.sql' }),
    });
    vi.mocked(save).mockResolvedValue('/tmp/prod.sql');

    renderBackup();
    await userEvent.click(screen.getByRole('button', { name: 'None' }));
    await userEvent.click(screen.getByRole('checkbox', { name: /Add DROP TABLE/ }));
    // Re-tick one so the primary button is enabled again.
    await userEvent.click(screen.getByRole('checkbox', { name: /^authors/ }));
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    await screen.findByText(/Backed up 1 tables/);
    const call = callsTo('backup_database')[0];
    expect(call?.options).toMatchObject({
      tables: [{ schema: null, name: 'authors' }],
      dropExisting: true,
    });
  });

  it('shows which table it is on while the backup runs', async () => {
    const progress = captureProgress<BackupProgress>();
    let finish: (r: unknown) => void = () => {};
    mockInvoke({ backup_database: () => new Promise((resolve) => (finish = resolve)) });
    vi.mocked(save).mockResolvedValue('/tmp/prod.sql');

    renderBackup();
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    // Before any event lands, the dialog still has to say something.
    expect(await screen.findByText('Starting…')).toBeInTheDocument();

    progress.emit({ table: 'books', tableIndex: 1, tableCount: 2, rowsWritten: 900 });
    expect(await screen.findByText(deepest(/books — 900 rows \(2 of 2\)/))).toBeInTheDocument();

    finish({ tables: 2, rows: 900, bytes: 10, path: '/tmp/prod.sql' });
    await screen.findByText(/Backed up 2 tables/);
  });

  it('surfaces a failed backup instead of looking like it worked', async () => {
    mockInvoke({ backup_database: faroError('io', 'permission denied writing /tmp/prod.sql') });
    vi.mocked(save).mockResolvedValue('/tmp/prod.sql');

    renderBackup();
    await userEvent.click(screen.getByRole('button', { name: /choose file/i }));

    expect(await screen.findByText(/permission denied/)).toBeInTheDocument();
    expect(screen.queryByText(/Backed up/)).not.toBeInTheDocument();
  });
});

describe('RestoreDialog — running a restore', () => {
  function renderRestore() {
    return render(
      <>
        <RestoreDialog
          open
          onClose={() => {}}
          connectionId="c1"
          connectionName="prod"
          onRestored={() => {}}
        />
        <ConfirmHost />
      </>,
    );
  }

  const fileInfo = {
    statements: 42,
    bytes: 2048,
    hasSchema: true,
    firstLines: ['-- Faro dump', 'CREATE TABLE authors ('],
  };

  async function chooseFile() {
    vi.mocked(openFile).mockResolvedValue('/tmp/dump.sql');
    await userEvent.click(screen.getByRole('button', { name: /browse/i }));
    return screen.findByText(deepest(/42 statements/));
  }

  it('inspects the chosen file and says what the dump will do', async () => {
    mockInvoke({ inspect_backup: () => fileInfo });
    renderRestore();

    await chooseFile();

    // The summary line, not the warning below it — both mention creating tables.
    expect(screen.getByText(deepest(/· creates tables/))).toBeInTheDocument();
    // The warning is the whole reason `hasSchema` is reported.
    expect(screen.getByText(deepest(/This dump creates tables/))).toBeInTheDocument();
    expect(callsTo('inspect_backup')[0]).toMatchObject({ path: '/tmp/dump.sql' });
  });

  it('names the target database in the confirmation and runs on accept', async () => {
    mockInvoke({
      inspect_backup: () => fileInfo,
      restore_database: () => ({ statements: 42, failed: 0, errors: [] }),
    });
    renderRestore();
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Restore' }));

    // Running a dump against the wrong database cannot be undone, so the
    // prompt has to name which one.
    expect(
      await screen.findByText(deepest(/Run 42 statements against "prod"\?/)),
    ).toBeInTheDocument();
    await userEvent.click(confirmButton('Restore'));

    expect(await screen.findByText('Ran 42 statements successfully.')).toBeInTheDocument();
    expect(callsTo('restore_database')[0]).toMatchObject({
      connectionId: 'c1',
      path: '/tmp/dump.sql',
      options: { stopOnError: true },
    });
  });

  it('writes nothing when the confirmation is declined', async () => {
    mockInvoke({
      inspect_backup: () => fileInfo,
      restore_database: () => ({ statements: 0, failed: 0, errors: [] }),
    });
    renderRestore();
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Restore' }));
    await screen.findByText(deepest(/Run 42 statements against "prod"\?/));
    await userEvent.click(confirmButton('Cancel'));

    expectCallCount('restore_database', 0);
  });

  it('reports a partial restore as partial rather than as success', async () => {
    mockInvoke({
      inspect_backup: () => fileInfo,
      restore_database: () => ({ statements: 42, failed: 3, errors: ['statement 7: boom'] }),
    });
    renderRestore();
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Restore' }));
    await screen.findByText(deepest(/Run 42 statements against "prod"\?/));
    await userEvent.click(confirmButton('Restore'));

    expect(await screen.findByText('Ran 42 statements; 3 failed.')).toBeInTheDocument();
  });

  it('passes the stop-on-error choice through', async () => {
    mockInvoke({
      inspect_backup: () => fileInfo,
      restore_database: () => ({ statements: 1, failed: 0, errors: [] }),
    });
    renderRestore();
    await chooseFile();

    await userEvent.click(screen.getByRole('checkbox', { name: /Stop at the first error/ }));
    await userEvent.click(screen.getByRole('button', { name: 'Restore' }));
    await screen.findByText(deepest(/Run 42 statements/));
    await userEvent.click(confirmButton('Restore'));

    await screen.findByText(/Ran 1 statements/);
    expect(callsTo('restore_database')[0]?.options).toMatchObject({ stopOnError: false });
  });

  it('surfaces a failed restore', async () => {
    mockInvoke({
      inspect_backup: () => fileInfo,
      restore_database: faroError('database', 'syntax error at or near "CRATE"'),
    });
    renderRestore();
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Restore' }));
    await screen.findByText(deepest(/Run 42 statements/));
    await userEvent.click(confirmButton('Restore'));

    expect(await screen.findByText(/syntax error/)).toBeInTheDocument();
  });

  it('registers a cancellable query id, so a long restore can be stopped', async () => {
    mockInvoke({
      inspect_backup: () => fileInfo,
      restore_database: () => ({ statements: 1, failed: 0, errors: [] }),
    });
    renderRestore();
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Restore' }));
    await screen.findByText(deepest(/Run 42 statements/));
    await userEvent.click(confirmButton('Restore'));

    await screen.findByText(/Ran 1 statements/);
    // `cancel_query` keys off this, which is the only way to stop a restore.
    expect(ipc.RESTORE_PROGRESS_EVENT).toBe('faro://restore-progress');
    expect(callsTo('restore_database')[0]?.queryId).toEqual(expect.stringMatching(/^res-\d+$/));
  });
});
