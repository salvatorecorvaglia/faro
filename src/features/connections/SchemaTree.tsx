import { useEffect, useMemo, useState } from 'react';

import { IconChevron, IconTable, IconView } from '@/components/icons';
import { ErrorBanner, FilterInput, rowActivation, Spinner } from '@/components/ui';
import * as ipc from '@/ipc';
import type { SchemaInfo, TableInfo } from '@/ipc/types';
import { useTabs } from '@/state/tabs';

export function SchemaTree({ connectionId }: { connectionId: string }) {
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

          {shown.map((t) => {
            const open = () => openTableTab(connectionId, { schema: t.schema, name: t.name });
            return (
              <div
                key={`${t.schema}.${t.name}`}
                className="flex h-6 cursor-pointer items-center gap-1.5 pl-9 pr-2 hover:bg-[var(--bg-inset)]"
                role="button"
                tabIndex={0}
                onKeyDown={rowActivation(open).onKeyDown}
                onClick={open}
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
            );
          })}
        </div>
      )}
    </div>
  );
}
