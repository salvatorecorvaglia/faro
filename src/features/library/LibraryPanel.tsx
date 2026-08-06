import { useEffect, useMemo, useState } from 'react';

import {
  IconChevron,
  IconClose,
  IconEdit,
  IconSearch,
  IconTrash,
  IconWarning,
} from '@/components/icons';
import { Spinner } from '@/components/ui';
import type { HistoryEntry, SavedQuery } from '@/ipc/types';
import { fuzzyRank, oneLine, relativeTime } from '@/lib/search';
import { formatDuration, formatRowCount } from '@/lib/value';
import { useConnections } from '@/state/connections';
import { groupByFolder, useLibrary } from '@/state/library';
import { useTabs } from '@/state/tabs';
import { SaveQueryDialog } from './SaveQueryDialog';

type LibraryTab = 'saved' | 'history';

/**
 * The saved-queries and history panel, below the connection tree.
 *
 * Opening an entry creates a new tab rather than overwriting the current one —
 * clobbering unsaved SQL to show an old query would be a poor trade.
 */
export function LibraryPanel() {
  const [tab, setTab] = useState<LibraryTab>('saved');
  const [collapsed, setCollapsed] = useState(false);
  const { saved, history, refreshSaved, refreshHistory, loading } = useLibrary();

  useEffect(() => {
    refreshSaved();
    refreshHistory();
  }, [refreshSaved, refreshHistory]);

  return (
    <div
      className="flex min-h-0 shrink-0 flex-col border-t"
      style={{
        borderColor: 'var(--border)',
        // Collapsed shows just the header bar; expanded takes a fixed slice so
        // the connection tree above keeps a usable amount of room.
        height: collapsed ? 30 : 280,
      }}
    >
      <div className="flex h-[30px] shrink-0 items-center gap-1 px-1.5">
        <button
          className="btn btn-ghost px-1"
          onClick={() => setCollapsed((c) => !c)}
          title={collapsed ? 'Expand' : 'Collapse'}
        >
          <IconChevron size={12} style={{ transform: collapsed ? undefined : 'rotate(90deg)' }} />
        </button>

        {(['saved', 'history'] as const).map((t) => (
          <button
            key={t}
            className="btn px-1.5 py-0.5 text-[11px]"
            onClick={() => {
              setTab(t);
              setCollapsed(false);
            }}
            style={
              tab === t && !collapsed
                ? { color: 'var(--accent)', background: 'var(--accent-soft)' }
                : { color: 'var(--text-muted)' }
            }
          >
            {t === 'saved' ? 'Saved' : 'History'}
            <span className="ml-1 text-[10px] tabular-nums opacity-70">
              {t === 'saved' ? saved.length : history.length}
            </span>
          </button>
        ))}

        <div className="flex-1" />
        {loading && <Spinner size={11} />}
      </div>

      {!collapsed && (
        <div className="min-h-0 flex-1">{tab === 'saved' ? <SavedList /> : <HistoryList />}</div>
      )}
    </div>
  );
}

function SavedList() {
  const { saved, remove } = useLibrary();
  const openQueryTab = useTabs((s) => s.openQueryTab);
  const [search, setSearch] = useState('');
  const [editing, setEditing] = useState<SavedQuery | null>(null);

  // Search matches the name, folder and SQL together, so a half-remembered
  // fragment of the query itself is enough to find it.
  const shown = useMemo(
    () =>
      fuzzyRank(saved, search, (q) => `${q.folder ?? ''} ${q.name} ${q.sql}`).map((r) => r.item),
    [saved, search],
  );

  const groups = useMemo(() => groupByFolder(shown), [shown]);

  return (
    <div className="flex h-full flex-col">
      <SearchBox value={search} onChange={setSearch} placeholder="Search saved queries" />

      <div className="min-h-0 flex-1 overflow-y-auto">
        {saved.length === 0 ? (
          <Hint text="No saved queries yet. Save one with ⌘S from a query tab." />
        ) : shown.length === 0 ? (
          <Hint text="Nothing matches." />
        ) : (
          groups.map((g) => (
            <div key={g.folder ?? '__loose'}>
              {g.folder && (
                <div
                  className="px-2 pt-1.5 pb-0.5 text-[10px] font-semibold uppercase tracking-wide"
                  style={{ color: 'var(--text-faint)' }}
                >
                  {g.folder}
                </div>
              )}
              {g.queries.map((q) => (
                <div
                  key={q.id}
                  className="group flex h-6 cursor-pointer items-center gap-1.5 px-2 hover:bg-[var(--bg-inset)]"
                  onClick={() => openQueryTab(q.connectionId, q.sql, q.name)}
                  title={q.sql}
                >
                  <span className="min-w-0 flex-1 truncate text-[11.5px]">{q.name}</span>
                  <div className="hidden shrink-0 items-center gap-0.5 group-hover:flex">
                    <button
                      className="btn btn-ghost px-1"
                      title="Rename"
                      onClick={(e) => {
                        e.stopPropagation();
                        setEditing(q);
                      }}
                    >
                      <IconEdit size={11} />
                    </button>
                    <button
                      className="btn btn-ghost px-1"
                      title="Delete"
                      onClick={(e) => {
                        e.stopPropagation();
                        if (confirm(`Delete the saved query "${q.name}"?`)) remove(q.id);
                      }}
                    >
                      <IconTrash size={11} />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          ))
        )}
      </div>

      <SaveQueryDialog
        open={!!editing}
        onClose={() => setEditing(null)}
        sql=""
        connectionId={null}
        editing={editing}
      />
    </div>
  );
}

function HistoryList() {
  const { history, historySearch, refreshHistory, clearHistory, deleteHistoryEntry } = useLibrary();
  const openQueryTab = useTabs((s) => s.openQueryTab);
  const connections = useConnections((s) => s.items);
  const [term, setTerm] = useState(historySearch);

  // History lives in SQL, not memory, so searching goes back to the store —
  // the table holds far more rows than the panel ever loads.
  useEffect(() => {
    const timer = setTimeout(() => refreshHistory(term), 200);
    return () => clearTimeout(timer);
  }, [term, refreshHistory]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-1 pr-1.5">
        <div className="min-w-0 flex-1">
          <SearchBox value={term} onChange={setTerm} placeholder="Search history" />
        </div>
        {history.length > 0 && (
          <button
            className="btn btn-ghost shrink-0 px-1"
            title="Clear all history"
            onClick={() => {
              if (confirm('Delete the entire query history?')) clearHistory();
            }}
          >
            <IconTrash size={11} />
          </button>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {history.length === 0 ? (
          <Hint text={term ? 'Nothing matches.' : 'Queries you run appear here.'} />
        ) : (
          history.map((h) => (
            <HistoryRow
              key={h.id}
              entry={h}
              // A connection may have been deleted since; fall back to the name
              // recorded at run time.
              stillExists={connections.some((c) => c.id === h.connectionId)}
              onOpen={() => openQueryTab(h.connectionId, h.sql)}
              onDelete={() => deleteHistoryEntry(h.id)}
            />
          ))
        )}
      </div>
    </div>
  );
}

function HistoryRow({
  entry,
  stillExists,
  onOpen,
  onDelete,
}: {
  entry: HistoryEntry;
  stillExists: boolean;
  onOpen: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      className="group flex cursor-pointer flex-col gap-0.5 border-b px-2 py-1 hover:bg-[var(--bg-inset)]"
      style={{ borderColor: 'var(--border)' }}
      onClick={onOpen}
      title={entry.sql}
    >
      <div className="flex items-center gap-1">
        {!entry.succeeded && (
          <IconWarning size={10} style={{ color: 'var(--danger)' }} className="shrink-0" />
        )}
        <span
          className="min-w-0 flex-1 truncate font-mono text-[11px]"
          style={{ color: entry.succeeded ? 'var(--text)' : 'var(--danger)' }}
        >
          {oneLine(entry.sql, 90)}
        </span>
        <button
          className="hidden shrink-0 opacity-60 hover:opacity-100 group-hover:block"
          title="Remove from history"
          onClick={(e) => {
            e.stopPropagation();
            onDelete();
          }}
        >
          <IconClose size={10} />
        </button>
      </div>
      <div className="flex items-center gap-1.5 text-[10px]" style={{ color: 'var(--text-faint)' }}>
        <span>{relativeTime(entry.executedAt)}</span>
        {entry.connectionName && (
          <>
            <span>·</span>
            <span
              className="truncate"
              style={{ textDecoration: stillExists ? undefined : 'line-through' }}
              title={stillExists ? undefined : 'This connection no longer exists'}
            >
              {entry.connectionName}
            </span>
          </>
        )}
        <span>·</span>
        <span className="tabular-nums">{formatDuration(entry.durationMs)}</span>
        {entry.succeeded && (
          <>
            <span>·</span>
            <span className="tabular-nums">{formatRowCount(entry.rowCount)} rows</span>
          </>
        )}
      </div>
    </div>
  );
}

function SearchBox({
  value,
  onChange,
  placeholder,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
}) {
  return (
    <div className="relative px-1.5 py-1">
      <IconSearch
        size={11}
        className="pointer-events-none absolute left-3.5 top-1/2 -translate-y-1/2 opacity-45"
      />
      <input
        className="input py-1 pl-6 text-[11px]"
        placeholder={placeholder}
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

function Hint({ text }: { text: string }) {
  return (
    <p className="px-3 py-4 text-center text-[11px]" style={{ color: 'var(--text-faint)' }}>
      {text}
    </p>
  );
}
