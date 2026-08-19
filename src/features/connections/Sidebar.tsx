import { useEffect, useMemo, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';

import {
  IconChevron,
  IconDatabase,
  IconDownload,
  IconEdit,
  IconLighthouse,
  IconPlus,
  IconRefresh,
  IconTable,
  IconTrash,
  IconUpload,
  IconView,
  IconWarning,
} from '@/components/icons';
import { ErrorBanner, FilterInput, rowActivation, Spinner } from '@/components/ui';
import { BackupDialog, RestoreDialog } from '@/features/backup/BackupDialog';
import { LibraryPanel } from '@/features/library/LibraryPanel';
import * as ipc from '@/ipc';
import type { ConnectionConfig, ConnectionStatus, SchemaInfo, TableInfo } from '@/ipc/types';
import { isSqlEngine, starterQuery } from '@/lib/engine';
import { useConnections } from '@/state/connections';
import { useSchemaCache } from '@/state/schemaCache';
import { useTabs } from '@/state/tabs';
import { ConnectionDialog } from './ConnectionDialog';

export function Sidebar() {
  // `useShallow` rather than a bare `useConnections()`: the latter subscribes
  // to the whole store object, so any unrelated field changing re-rendered the
  // entire connection tree.
  const {
    items,
    loading,
    connecting,
    error,
    keychainOk,
    refresh,
    connect,
    disconnect,
    remove,
    clearError,
  } = useConnections(
    useShallow((s) => ({
      items: s.items,
      loading: s.loading,
      connecting: s.connecting,
      error: s.error,
      keychainOk: s.keychainOk,
      refresh: s.refresh,
      connect: s.connect,
      disconnect: s.disconnect,
      remove: s.remove,
      clearError: s.clearError,
    })),
  );
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<ConnectionConfig | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [backupFor, setBackupFor] = useState<ConnectionStatus | null>(null);
  const [restoreFor, setRestoreFor] = useState<ConnectionStatus | null>(null);
  /** Table list for whichever connection a backup dialog is open on. */
  const [backupTables, setBackupTables] = useState<TableInfo[]>([]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Fetch the table list when the backup dialog opens rather than keeping one
  // for every connection: only the open dialog needs it.
  useEffect(() => {
    if (!backupFor) {
      setBackupTables([]);
      return;
    }
    let cancelled = false;
    ipc
      .listTables(backupFor.id, null)
      .then((t) => !cancelled && setBackupTables(t))
      .catch(() => !cancelled && setBackupTables([]));
    return () => {
      cancelled = true;
    };
  }, [backupFor]);

  function openNew() {
    setEditing(null);
    setDialogOpen(true);
  }

  function openEdit(c: ConnectionConfig) {
    setEditing(c);
    setDialogOpen(true);
  }

  async function toggle(c: ConnectionStatus) {
    if (expanded.has(c.id)) {
      setExpanded((prev) => {
        const next = new Set(prev);
        next.delete(c.id);
        return next;
      });
      return;
    }
    if (!c.connected) {
      const ok = await connect(c.id);
      if (!ok) return;
    }
    // Functional update, because connecting is awaited: a snapshot of
    // `expanded` taken before the await would clobber any other connection the
    // user expanded while this one was still opening.
    setExpanded((prev) => new Set(prev).add(c.id));
  }

  return (
    <div className="flex h-full flex-col" style={{ background: 'var(--bg-subtle)' }}>
      <header
        className="flex h-9 shrink-0 items-center gap-1.5 border-b px-2.5"
        style={{ borderColor: 'var(--border)' }}
      >
        <IconLighthouse size={15} className="opacity-90" />
        <span className="flex-1 text-[12px] font-semibold tracking-wide">Faro</span>
        <button type="button" className="btn btn-ghost px-1.5" onClick={refresh} title="Refresh">
          <IconRefresh size={13} />
        </button>
        <button
          type="button"
          className="btn btn-ghost px-1.5"
          onClick={openNew}
          title="New connection"
        >
          <IconPlus size={14} />
        </button>
      </header>

      {!keychainOk && (
        <div
          className="flex items-start gap-1.5 px-2.5 py-1.5 text-[11px]"
          style={{ color: 'var(--warning)' }}
        >
          <IconWarning size={12} className="mt-0.5 shrink-0" />
          <span>No system keychain found. Passwords will only be kept until you quit.</span>
        </div>
      )}

      {error && (
        <div className="px-2 py-1.5">
          <ErrorBanner message={error} onDismiss={clearError} />
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-y-auto py-1">
        {loading ? (
          <div
            className="flex items-center gap-2 px-3 py-2 text-[12px]"
            style={{ color: 'var(--text-muted)' }}
          >
            <Spinner /> Loading…
          </div>
        ) : items.length === 0 ? (
          <div className="px-3 py-6 text-center">
            <p className="text-[12px]" style={{ color: 'var(--text-muted)' }}>
              No connections yet
            </p>
            <button type="button" className="btn btn-primary mt-2.5" onClick={openNew}>
              <IconPlus size={13} /> Add connection
            </button>
          </div>
        ) : (
          items.map((c) => (
            <ConnectionNode
              key={c.id}
              conn={c}
              expanded={expanded.has(c.id)}
              connecting={connecting.has(c.id)}
              onToggle={() => toggle(c)}
              onEdit={() => openEdit(c)}
              onDisconnect={() => disconnect(c.id)}
              onRemove={() => remove(c.id)}
              onBackup={() => setBackupFor(c)}
              onRestore={() => setRestoreFor(c)}
            />
          ))
        )}
      </div>

      <LibraryPanel />

      <ConnectionDialog open={dialogOpen} onClose={() => setDialogOpen(false)} editing={editing} />

      <BackupDialog
        open={!!backupFor}
        onClose={() => setBackupFor(null)}
        connectionId={backupFor?.id ?? null}
        connectionName={backupFor?.name ?? ''}
        tables={backupTables}
      />

      <RestoreDialog
        open={!!restoreFor}
        onClose={() => setRestoreFor(null)}
        connectionId={restoreFor?.id ?? null}
        connectionName={restoreFor?.name ?? ''}
        // A restore changes the schema, so the cached one is no longer valid.
        onRestored={() => {
          if (restoreFor) useSchemaCache.getState().invalidate(restoreFor.id);
        }}
      />
    </div>
  );
}

function ConnectionNode({
  conn,
  expanded,
  connecting,
  onToggle,
  onEdit,
  onDisconnect,
  onRemove,
  onBackup,
  onRestore,
}: {
  conn: ConnectionStatus;
  expanded: boolean;
  connecting: boolean;
  onToggle: () => void;
  onEdit: () => void;
  onDisconnect: () => void;
  onRemove: () => void;
  onBackup: () => void;
  onRestore: () => void;
}) {
  const openQueryTab = useTabs((s) => s.openQueryTab);

  return (
    <div>
      <div
        className="group flex h-7 cursor-pointer items-center gap-1 px-1.5 hover:bg-[var(--bg-inset)]"
        role="button"
        tabIndex={0}
        onKeyDown={rowActivation(onToggle).onKeyDown}
        onClick={onToggle}
      >
        <IconChevron
          size={12}
          className="shrink-0 transition-transform"
          style={{ transform: expanded ? 'rotate(90deg)' : undefined } as React.CSSProperties}
        />
        <span
          className="h-1.5 w-1.5 shrink-0 rounded-full"
          style={{
            background: conn.connected ? 'var(--success)' : 'var(--border-strong)',
          }}
          title={conn.connected ? 'Connected' : 'Not connected'}
        />
        {conn.color && (
          <span className="h-3 w-0.5 shrink-0 rounded-full" style={{ background: conn.color }} />
        )}
        <IconDatabase size={13} className="shrink-0 opacity-60" />
        <span className="min-w-0 flex-1 truncate text-[12px]">{conn.name}</span>

        {connecting && <Spinner size={12} />}

        <div className="hidden shrink-0 items-center gap-0.5 group-hover:flex">
          {conn.connected && (
            <button
              type="button"
              className="btn btn-ghost px-1"
              title="New query"
              onClick={(e) => {
                e.stopPropagation();
                openQueryTab(conn.id, starterQuery(conn.engine));
              }}
            >
              <IconPlus size={12} />
            </button>
          )}
          {conn.connected && isSqlEngine(conn.engine) && (
            <>
              <button
                type="button"
                className="btn btn-ghost px-1"
                title="Back up this database"
                onClick={(e) => {
                  e.stopPropagation();
                  onBackup();
                }}
              >
                <IconDownload size={12} />
              </button>
              {/* Restore writes, so it is unavailable on a read-only
                  connection. The backend refuses it too; hiding the affordance
                  saves the user picking a file first. */}
              <button
                type="button"
                className="btn btn-ghost px-1"
                disabled={conn.readOnly}
                title={
                  conn.readOnly
                    ? 'This connection is open read-only'
                    : 'Restore a dump into this database'
                }
                onClick={(e) => {
                  e.stopPropagation();
                  onRestore();
                }}
              >
                <IconUpload size={12} />
              </button>
            </>
          )}
          <button
            type="button"
            className="btn btn-ghost px-1"
            title="Edit"
            onClick={(e) => {
              e.stopPropagation();
              onEdit();
            }}
          >
            <IconEdit size={12} />
          </button>
          <button
            type="button"
            className="btn btn-ghost px-1"
            title={conn.connected ? 'Disconnect' : 'Delete'}
            onClick={(e) => {
              e.stopPropagation();
              if (conn.connected) {
                onDisconnect();
              } else if (confirm(`Delete the connection "${conn.name}"?`)) {
                onRemove();
              }
            }}
          >
            <IconTrash size={12} />
          </button>
        </div>
      </div>

      {expanded && conn.connected && <SchemaTree connectionId={conn.id} />}
    </div>
  );
}

/** Schemas and their tables for one connected database. */
function SchemaTree({ connectionId }: { connectionId: string }) {
  const [schemas, setSchemas] = useState<SchemaInfo[] | null>(null);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    ipc
      .listSchemas(connectionId)
      .then((s) => {
        if (cancelled) return;
        setSchemas(s);
        // Auto-expand the first non-system schema: on Postgres that is almost
        // always `public`, and it saves a click on the common path.
        const first = s.find((x) => !x.isSystem);
        if (first) setOpen(new Set([first.name]));
      })
      .catch((e) => !cancelled && setError(ipc.errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [connectionId]);

  if (error) {
    return (
      <div className="px-2 py-1">
        <ErrorBanner message={error} />
      </div>
    );
  }
  if (!schemas) {
    return (
      <div
        className="flex items-center gap-2 py-1 pl-7 text-[11px]"
        style={{ color: 'var(--text-faint)' }}
      >
        <Spinner size={11} /> Loading schemas…
      </div>
    );
  }

  // System schemas sort last: they are rarely what the user came for.
  const ordered = [...schemas].sort(
    (a, b) => Number(a.isSystem) - Number(b.isSystem) || a.name.localeCompare(b.name),
  );

  return (
    <div>
      {ordered.map((s) => (
        <SchemaNode
          key={s.name}
          connectionId={connectionId}
          schema={s}
          open={open.has(s.name)}
          onToggle={() =>
            setOpen((prev) => {
              const next = new Set(prev);
              next.has(s.name) ? next.delete(s.name) : next.add(s.name);
              return next;
            })
          }
        />
      ))}
    </div>
  );
}

function SchemaNode({
  connectionId,
  schema,
  open,
  onToggle,
}: {
  connectionId: string;
  schema: SchemaInfo;
  open: boolean;
  onToggle: () => void;
}) {
  const [tables, setTables] = useState<TableInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const openTableTab = useTabs((s) => s.openTableTab);

  useEffect(() => {
    if (!open || tables) return;
    ipc
      .listTables(connectionId, schema.name)
      .then(setTables)
      .catch((e) => setError(ipc.errorMessage(e)));
  }, [open, tables, connectionId, schema.name]);

  const shown = useMemo(() => {
    if (!tables) return [];
    const q = filter.trim().toLowerCase();
    return q ? tables.filter((t) => t.name.toLowerCase().includes(q)) : tables;
  }, [tables, filter]);

  // A filter box only earns its space once the list is long enough to need it.
  const showFilter = (tables?.length ?? 0) > 12;

  return (
    <div>
      <div
        className="flex h-6 cursor-pointer items-center gap-1 pl-5 pr-2 hover:bg-[var(--bg-inset)]"
        role="button"
        tabIndex={0}
        onKeyDown={rowActivation(onToggle).onKeyDown}
        onClick={onToggle}
      >
        <IconChevron
          size={11}
          className="shrink-0 transition-transform"
          style={{ transform: open ? 'rotate(90deg)' : undefined } as React.CSSProperties}
        />
        <span
          className="min-w-0 flex-1 truncate text-[11.5px]"
          style={{ color: schema.isSystem ? 'var(--text-faint)' : 'var(--text-muted)' }}
        >
          {schema.name}
        </span>
        {tables && (
          <span className="text-[10px] tabular-nums" style={{ color: 'var(--text-faint)' }}>
            {tables.length}
          </span>
        )}
      </div>

      {open && (
        <div>
          {error && (
            <div className="px-2 py-1">
              <ErrorBanner message={error} />
            </div>
          )}
          {!tables && !error && (
            <div
              className="flex items-center gap-2 py-1 pl-9 text-[11px]"
              style={{ color: 'var(--text-faint)' }}
            >
              <Spinner size={10} /> Loading…
            </div>
          )}

          {showFilter && (
            <FilterInput
              value={filter}
              onChange={setFilter}
              placeholder="Filter tables"
              wrapperClassName="relative px-2 py-1 pl-8"
              iconClassName="pointer-events-none absolute left-9 top-1/2 -translate-y-1/2 opacity-45"
            />
          )}

          {tables?.length === 0 && (
            <p className="py-1 pl-9 text-[11px]" style={{ color: 'var(--text-faint)' }}>
              No tables
            </p>
          )}

          {shown.map((t) => (
            <div
              key={`${t.schema}.${t.name}`}
              className="flex h-6 cursor-pointer items-center gap-1.5 pl-9 pr-2 hover:bg-[var(--bg-inset)]"
              onClick={() => openTableTab(connectionId, { schema: t.schema, name: t.name })}
              title={
                t.estimatedRows != null ? `~${t.estimatedRows.toLocaleString()} rows` : undefined
              }
            >
              {t.kind === 'table' ? (
                <IconTable size={12} className="shrink-0 opacity-55" />
              ) : (
                <IconView size={12} className="shrink-0 opacity-55" />
              )}
              <span className="min-w-0 flex-1 truncate text-[11.5px]">{t.name}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
