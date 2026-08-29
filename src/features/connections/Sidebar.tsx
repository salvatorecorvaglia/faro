import { useEffect, useState } from 'react';
import { useShallow } from 'zustand/react/shallow';

import {
  IconChevron,
  IconDatabase,
  IconDownload,
  IconEdit,
  IconLighthouse,
  IconPlus,
  IconRefresh,
  IconTrash,
  IconUpload,
  IconWarning,
} from '@/components/icons';
import { ErrorBanner, rowActivation, Spinner } from '@/components/ui';
import { BackupDialog, RestoreDialog } from '@/features/backup/BackupDialog';
import { LibraryPanel } from '@/features/library/LibraryPanel';
import * as ipc from '@/ipc';
import type { ConnectionConfig, ConnectionStatus, TableInfo } from '@/ipc/types';
import { isSqlEngine, starterQuery } from '@/lib/engine';
import { confirmDialog } from '@/state/confirm';
import { useConnections } from '@/state/connections';
import { useSchemaCache } from '@/state/schemaCache';
import { useTabs } from '@/state/tabs';
import { ConnectionDialog } from './ConnectionDialog';
import { SchemaTree } from './SchemaTree';

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
            onClick={async (e) => {
              e.stopPropagation();
              if (conn.connected) {
                onDisconnect();
                return;
              }
              const proceed = await confirmDialog(`Delete the connection "${conn.name}"?`, {
                confirmLabel: 'Delete',
                danger: true,
              });
              if (proceed) onRemove();
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
