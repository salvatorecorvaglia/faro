import { useEffect, useMemo, useRef, useState } from 'react';

import {
  IconDatabase,
  IconPlay,
  IconPlus,
  IconSearch,
  IconTable,
  IconWarning,
} from '@/components/icons';
import { fuzzyRank, oneLine, relativeTime } from '@/lib/search';
import { useConnections } from '@/state/connections';
import { useLibrary } from '@/state/library';
import { useSchemaCache } from '@/state/schemaCache';
import { useTabs } from '@/state/tabs';

interface Item {
  id: string;
  /** Text the fuzzy matcher searches. */
  search: string;
  label: string;
  detail?: string;
  group: string;
  icon: React.ReactNode;
  danger?: boolean;
  run: () => void;
}

/**
 * ⌘K palette over everything in the app: saved queries, history, tables,
 * connections and actions.
 *
 * With no query typed it shows a useful default set rather than nothing, so it
 * doubles as a launcher. Results are capped because the list is not
 * virtualized — beyond a screenful or two nobody scrolls anyway, they type.
 */
export function CommandPalette({
  open,
  onClose,
  onShowShortcuts,
}: {
  open: boolean;
  onClose: () => void;
  onShowShortcuts?: () => void;
}) {
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const connections = useConnections((s) => s.items);
  const connect = useConnections((s) => s.connect);
  const saved = useLibrary((s) => s.saved);
  const history = useLibrary((s) => s.history);
  const schemaByConnection = useSchemaCache((s) => s.byConnection);
  const { openQueryTab, openTableTab, activeTab } = useTabs();

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setCursor(0);
  }, [open]);

  const items = useMemo<Item[]>(() => {
    const out: Item[] = [];
    const current = activeTab();

    if (onShowShortcuts) {
      out.push({
        id: 'action:shortcuts',
        search: 'keyboard shortcuts help keys',
        label: 'Keyboard shortcuts',
        detail: '?',
        group: 'Actions',
        icon: <IconSearch size={12} />,
        run: onShowShortcuts,
      });
    }

    out.push({
      id: 'action:new-query',
      search: 'new query tab',
      label: 'New query tab',
      group: 'Actions',
      icon: <IconPlus size={12} />,
      run: () =>
        openQueryTab(current?.connectionId ?? connections.find((c) => c.connected)?.id ?? null),
    });

    for (const c of connections) {
      out.push({
        id: `conn:${c.id}`,
        search: `${c.name} ${c.host} ${c.database} connection`,
        label: c.name,
        detail: c.connected ? 'connected' : 'connect',
        group: 'Connections',
        icon: <IconDatabase size={12} />,
        run: () => (c.connected ? openQueryTab(c.id) : connect(c.id)),
      });
    }

    for (const q of saved) {
      out.push({
        id: `saved:${q.id}`,
        search: `${q.folder ?? ''} ${q.name} ${q.sql}`,
        label: q.name,
        detail: q.folder ?? oneLine(q.sql, 50),
        group: 'Saved queries',
        icon: <IconPlay size={11} />,
        run: () => openQueryTab(q.connectionId, q.sql, q.name),
      });
    }

    // Tables come from whichever connections have been browsed, so the palette
    // never blocks on a schema fetch just to open.
    for (const [connectionId, tables] of Object.entries(schemaByConnection)) {
      const conn = connections.find((c) => c.id === connectionId);
      if (!conn?.connected) continue;
      for (const t of tables) {
        out.push({
          id: `table:${connectionId}:${t.schema ?? ''}:${t.name}`,
          search: `${t.name} ${t.schema ?? ''} ${conn.name} table`,
          label: t.name,
          detail: `${conn.name}${t.schema ? ` · ${t.schema}` : ''}`,
          group: 'Tables',
          icon: <IconTable size={11} />,
          run: () => openTableTab(connectionId, { schema: t.schema, name: t.name }),
        });
      }
    }

    for (const h of history) {
      out.push({
        id: `history:${h.id}`,
        search: h.sql,
        label: oneLine(h.sql, 70),
        detail: `${relativeTime(h.executedAt)}${h.connectionName ? ` · ${h.connectionName}` : ''}`,
        group: 'History',
        icon: h.succeeded ? <IconPlay size={11} /> : <IconWarning size={11} />,
        danger: !h.succeeded,
        run: () => openQueryTab(h.connectionId, h.sql),
      });
    }

    return out;
  }, [
    connections,
    saved,
    history,
    schemaByConnection,
    openQueryTab,
    openTableTab,
    connect,
    activeTab,
    onShowShortcuts,
  ]);

  const results = useMemo(() => {
    const ranked = fuzzyRank(items, query, (i) => i.search).map((r) => r.item);
    return ranked.slice(0, 60);
  }, [items, query]);

  // Clamp rather than reset, so narrowing the query does not always jump the
  // selection back to the top.
  useEffect(() => {
    setCursor((c) => Math.min(c, Math.max(0, results.length - 1)));
  }, [results.length]);

  useEffect(() => {
    listRef.current
      ?.querySelector<HTMLElement>(`[data-index="${cursor}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [cursor]);

  if (!open) return null;

  function choose(item: Item | undefined) {
    if (!item) return;
    item.run();
    onClose();
  }

  function onKeyDown(e: React.KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setCursor((c) => (c + 1) % Math.max(1, results.length));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setCursor((c) => (c - 1 + results.length) % Math.max(1, results.length));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      choose(results[cursor]);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  }

  let lastGroup = '';

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center pt-[12vh]"
      style={{ background: 'rgb(0 0 0 / 0.4)' }}
      onClick={onClose}
    >
      <div
        className="flex max-h-[60vh] w-[560px] max-w-[92vw] flex-col overflow-hidden rounded-xl"
        style={{
          background: 'var(--bg)',
          border: '1px solid var(--border-strong)',
          boxShadow: '0 16px 48px rgb(0 0 0 / 0.28)',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          className="flex shrink-0 items-center gap-2 border-b px-3"
          style={{ borderColor: 'var(--border)' }}
        >
          <IconSearch size={14} className="opacity-45" />
          <input
            className="flex-1 bg-transparent py-3 text-[13px] outline-none"
            style={{ color: 'var(--text)' }}
            placeholder="Search queries, tables, connections…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKeyDown}
            autoFocus
          />
          <kbd
            className="rounded px-1.5 py-0.5 text-[10px]"
            style={{ background: 'var(--bg-inset)', color: 'var(--text-faint)' }}
          >
            esc
          </kbd>
        </div>

        <div ref={listRef} className="min-h-0 flex-1 overflow-y-auto py-1">
          {results.length === 0 ? (
            <p className="px-3 py-6 text-center text-[12px]" style={{ color: 'var(--text-faint)' }}>
              Nothing matches “{query}”.
            </p>
          ) : (
            results.map((item, i) => {
              const showGroup = item.group !== lastGroup;
              lastGroup = item.group;
              const active = i === cursor;
              return (
                <div key={item.id}>
                  {showGroup && (
                    <div
                      className="px-3 pt-2 pb-1 text-[10px] font-semibold uppercase tracking-wide"
                      style={{ color: 'var(--text-faint)' }}
                    >
                      {item.group}
                    </div>
                  )}
                  <div
                    data-index={i}
                    className="mx-1 flex cursor-pointer items-center gap-2 rounded-md px-2 py-1.5"
                    style={{
                      background: active ? 'var(--accent)' : undefined,
                      color: active ? '#fff' : undefined,
                    }}
                    onMouseEnter={() => setCursor(i)}
                    onClick={() => choose(item)}
                  >
                    <span
                      className="shrink-0"
                      style={{
                        color: active
                          ? 'rgb(255 255 255 / 0.85)'
                          : item.danger
                            ? 'var(--danger)'
                            : 'var(--text-faint)',
                      }}
                    >
                      {item.icon}
                    </span>
                    <span className="min-w-0 flex-1 truncate text-[12.5px]">{item.label}</span>
                    {item.detail && (
                      <span
                        className="shrink-0 truncate text-[11px]"
                        style={{
                          maxWidth: '45%',
                          color: active ? 'rgb(255 255 255 / 0.7)' : 'var(--text-faint)',
                        }}
                      >
                        {item.detail}
                      </span>
                    )}
                  </div>
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
