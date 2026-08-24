import { save } from '@tauri-apps/plugin-dialog';
import { useEffect, useMemo, useRef, useState } from 'react';

import { ErrorBanner, Field, FilterInput, Modal, Spinner } from '@/components/ui';
import { useAsyncAction, useBackendProgress } from '@/hooks/useAsyncAction';
import * as ipc from '@/ipc';
import type { BackupProgress, TableInfo } from '@/ipc/types';
import { formatBytes, formatRowCount } from '@/lib/value';

let backupCounter = 0;
let restoreCounter = 0;

/**
 * Back up the connected database to a `.sql` file.
 *
 * Tables default to all-selected: someone opening a backup dialog almost always
 * wants the whole database, and having to tick every box to get that would be
 * a poor default.
 */
export function BackupDialog({
  open,
  onClose,
  connectionId,
  connectionName,
  tables,
}: {
  open: boolean;
  onClose: () => void;
  connectionId: string | null;
  connectionName: string;
  tables: TableInfo[];
}) {
  // Memoized so the seeding effect below keys off the actual list rather than
  // a fresh array identity on every render.
  const dataTables = useMemo(() => tables.filter((t) => t.kind === 'table'), [tables]);

  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState('');
  const [includeSchema, setIncludeSchema] = useState(true);
  const [includeData, setIncludeData] = useState(true);
  const [dropExisting, setDropExisting] = useState(false);
  const { busy, error, done, reset, run } = useAsyncAction();
  const progress = useBackendProgress<BackupProgress>(ipc.BACKUP_PROGRESS_EVENT, busy);

  /** Whether the selection has been seeded for the current opening. */
  const seeded = useRef(false);

  useEffect(() => {
    if (!open) {
      // Arm the next opening to seed again.
      seeded.current = false;
      return;
    }
    reset();
    setFilter('');
  }, [open, reset]);

  // Seed the selection once the table list has actually arrived.
  //
  // The sidebar starts fetching tables only after it sets the dialog open, so
  // on the first render the list is always empty. Seeding on `open` alone
  // therefore selected nothing and left the primary button disabled, and the
  // header read "0 of N" once the list landed.
  //
  // Guarded by a ref rather than keyed on `open` so that re-seeding happens
  // exactly once per opening — re-running it on every change to `dataTables`
  // would fight the user's own ticking, which is what the original comment was
  // protecting against.
  useEffect(() => {
    if (!open || seeded.current || dataTables.length === 0) return;
    seeded.current = true;
    setSelected(new Set(dataTables.map((t) => t.name)));
  }, [open, dataTables]);

  const shown = filter.trim()
    ? dataTables.filter((t) => t.name.toLowerCase().includes(filter.trim().toLowerCase()))
    : dataTables;

  function toggle(name: string) {
    setSelected((s) => {
      const next = new Set(s);
      next.has(name) ? next.delete(name) : next.add(name);
      return next;
    });
  }

  async function runBackup() {
    if (!connectionId) return;
    await run(async () => {
      const stem = connectionName.replace(/[^A-Za-z0-9_-]+/g, '_') || 'backup';
      const stamp = new Date().toISOString().slice(0, 10);
      const path = await save({
        defaultPath: `${stem}_${stamp}.sql`,
        filters: [{ name: 'SQL', extensions: ['sql'] }],
      });
      if (!path) return;

      const result = await ipc.backupDatabase(
        connectionId,
        path,
        {
          // An empty list means "everything" to the backend, but being
          // explicit keeps the dump reproducible from what the user actually
          // saw.
          tables: dataTables
            .filter((t) => selected.has(t.name))
            .map((t) => ({ schema: t.schema, name: t.name })),
          includeSchema,
          includeData,
          dropExisting,
        },
        `bak-${++backupCounter}`,
      );
      return (
        `Backed up ${result.tables} tables, ${formatRowCount(result.rows)} rows ` +
        `(${formatBytes(result.bytes)}) to ${result.path}`
      );
    });
  }

  return (
    <Modal open={open} onClose={onClose} title={`Back up ${connectionName}`} width={520}>
      <div className="flex flex-col gap-3">
        <div className="flex flex-col gap-1.5">
          <label className="flex items-center gap-2 text-[12px]">
            <input
              type="checkbox"
              checked={includeSchema}
              onChange={(e) => setIncludeSchema(e.target.checked)}
            />
            Include schema (CREATE TABLE, indexes, foreign keys)
          </label>
          <label className="flex items-center gap-2 text-[12px]">
            <input
              type="checkbox"
              checked={includeData}
              onChange={(e) => setIncludeData(e.target.checked)}
            />
            Include data (INSERT statements)
          </label>
          <label className="flex items-center gap-2 text-[12px]">
            <input
              type="checkbox"
              checked={dropExisting}
              onChange={(e) => setDropExisting(e.target.checked)}
              disabled={!includeSchema}
            />
            Add DROP TABLE before each CREATE
            <span style={{ color: 'var(--text-faint)' }}>
              — makes the dump overwrite an existing database
            </span>
          </label>
        </div>

        <div>
          <div className="mb-1 flex items-center justify-between">
            <span className="label mb-0">
              Tables — {selected.size} of {dataTables.length}
            </span>
            <div className="flex gap-1">
              <button
                className="btn btn-ghost text-[11px]"
                onClick={() => setSelected(new Set(dataTables.map((t) => t.name)))}
                type="button"
              >
                All
              </button>
              <button
                className="btn btn-ghost text-[11px]"
                onClick={() => setSelected(new Set())}
                type="button"
              >
                None
              </button>
            </div>
          </div>

          {dataTables.length > 10 && (
            <FilterInput
              value={filter}
              onChange={setFilter}
              placeholder="Filter tables"
              wrapperClassName="relative mb-1"
              iconClassName="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 opacity-45"
            />
          )}

          <div
            className="max-h-48 overflow-auto rounded-md border p-1"
            style={{ borderColor: 'var(--border)' }}
          >
            {shown.length === 0 ? (
              <p
                className="px-2 py-3 text-center text-[11px]"
                style={{ color: 'var(--text-faint)' }}
              >
                {dataTables.length === 0 ? 'No tables to back up.' : 'Nothing matches.'}
              </p>
            ) : (
              shown.map((t) => (
                <label
                  key={t.name}
                  className="flex cursor-pointer items-center gap-2 rounded px-1.5 py-0.5 text-[11.5px] hover:bg-[var(--bg-inset)]"
                >
                  <input
                    type="checkbox"
                    checked={selected.has(t.name)}
                    onChange={() => toggle(t.name)}
                  />
                  <span className="min-w-0 flex-1 truncate">{t.name}</span>
                  {t.estimatedRows != null && (
                    <span
                      className="text-[10px] tabular-nums"
                      style={{ color: 'var(--text-faint)' }}
                    >
                      ~{formatRowCount(t.estimatedRows)}
                    </span>
                  )}
                </label>
              ))
            )}
          </div>
        </div>

        {busy && (
          <div
            className="flex items-center gap-2 text-[12px]"
            style={{ color: 'var(--text-muted)' }}
          >
            <Spinner size={12} />
            {progress ? (
              <span>
                {progress.table} — {formatRowCount(progress.rowsWritten)} rows (
                {progress.tableIndex + 1} of {progress.tableCount})
              </span>
            ) : (
              <span>Starting…</span>
            )}
          </div>
        )}

        {error && <ErrorBanner message={error} onDismiss={reset} />}
        {done && (
          <p className="break-all text-[11.5px]" style={{ color: 'var(--success)' }}>
            {done}
          </p>
        )}

        <div className="mt-1 flex justify-end gap-2">
          <button className="btn btn-ghost" onClick={onClose} type="button">
            Close
          </button>
          <button
            className="btn btn-primary"
            onClick={runBackup}
            disabled={busy || selected.size === 0 || (!includeSchema && !includeData)}
            type="button"
          >
            {busy && <Spinner size={11} />}
            Choose file…
          </button>
        </div>
      </div>
    </Modal>
  );
}

/**
 * Restore a `.sql` dump into the connected database.
 *
 * The file is inspected before it runs and the target connection is named in the
 * confirmation, because running a dump against the wrong database is both easy
 * to do and impossible to undo.
 */
export function RestoreDialog({
  open,
  onClose,
  connectionId,
  connectionName,
  onRestored,
}: {
  open: boolean;
  onClose: () => void;
  connectionId: string | null;
  connectionName: string;
  onRestored: () => void;
}) {
  const [path, setPath] = useState<string | null>(null);
  const [info, setInfo] = useState<ipc.BackupFileInfo | null>(null);
  const [stopOnError, setStopOnError] = useState(true);
  const { busy, error, done, reset: resetStatus, run } = useAsyncAction();
  const progress = useBackendProgress<{ done: number; total: number }>(
    ipc.RESTORE_PROGRESS_EVENT,
    busy,
  );

  useEffect(() => {
    if (open) return;
    setPath(null);
    setInfo(null);
    resetStatus();
  }, [open, resetStatus]);

  async function choose() {
    const { open: openFile } = await import('@tauri-apps/plugin-dialog');
    const picked = await openFile({
      multiple: false,
      filters: [
        { name: 'SQL dump', extensions: ['sql'] },
        { name: 'All files', extensions: ['*'] },
      ],
    });
    if (typeof picked !== 'string') return;

    setPath(picked);
    setInfo(null);
    await run(async () => {
      setInfo(await ipc.inspectBackup(picked));
    });
  }

  async function runRestore() {
    if (!path || !connectionId) return;
    // Naming the target explicitly: this is the step that cannot be undone.
    if (
      !confirm(
        `Run ${info?.statements ?? '?'} statements against "${connectionName}"?\n\n` +
          `This modifies that database and cannot be undone.`,
      )
    ) {
      return;
    }

    const ok = await run(async () => {
      const result = await ipc.restoreDatabase(
        connectionId,
        path,
        { stopOnError },
        `res-${++restoreCounter}`,
      );
      return result.failed === 0
        ? `Ran ${result.statements} statements successfully.`
        : `Ran ${result.statements} statements; ${result.failed} failed.`;
    });
    if (ok) onRestored();
  }

  return (
    <Modal open={open} onClose={onClose} title={`Restore into ${connectionName}`} width={520}>
      <div className="flex flex-col gap-3">
        <div className="flex items-end gap-2">
          <div className="min-w-0 flex-1">
            <Field label="Dump file">
              <input
                className="input truncate font-mono"
                value={path ?? ''}
                placeholder="Choose a .sql file"
                readOnly
              />
            </Field>
          </div>
          <button className="btn btn-outline" onClick={choose} disabled={busy} type="button">
            Browse…
          </button>
        </div>

        {info && (
          <div
            className="rounded-md p-2 text-[11.5px]"
            style={{ background: 'var(--bg-inset)', color: 'var(--text-muted)' }}
          >
            <p>
              {info.statements} statements · {formatBytes(info.bytes)}
              {info.hasSchema ? ' · creates tables' : ' · data only'}
            </p>
            {info.hasSchema && (
              <p className="mt-1" style={{ color: 'var(--warning)' }}>
                This dump creates tables. If they already exist, those statements will fail unless
                the dump drops them first.
              </p>
            )}
            <pre className="selectable mt-1.5 max-h-20 overflow-auto font-mono text-[10.5px]">
              {info.firstLines.join('\n')}
            </pre>
          </div>
        )}

        <label className="flex items-center gap-2 text-[12px]">
          <input
            type="checkbox"
            checked={stopOnError}
            onChange={(e) => setStopOnError(e.target.checked)}
          />
          Stop at the first error
          <span style={{ color: 'var(--text-faint)' }}>— recommended</span>
        </label>

        {busy && (
          <div
            className="flex items-center gap-2 text-[12px]"
            style={{ color: 'var(--text-muted)' }}
          >
            <Spinner size={12} />
            <span>
              {progress ? `${progress.done} of ${progress.total} statements` : 'Starting…'}
            </span>
          </div>
        )}

        {error && <ErrorBanner message={error} onDismiss={resetStatus} />}
        {done && (
          <p className="text-[12px]" style={{ color: 'var(--success)' }}>
            {done}
          </p>
        )}

        <div className="mt-1 flex justify-end gap-2">
          <button className="btn btn-ghost" onClick={onClose} type="button">
            Close
          </button>
          <button
            className="btn btn-primary"
            onClick={runRestore}
            disabled={busy || !path || !info}
            type="button"
          >
            {busy && <Spinner size={11} />}
            Restore
          </button>
        </div>
      </div>
    </Modal>
  );
}
