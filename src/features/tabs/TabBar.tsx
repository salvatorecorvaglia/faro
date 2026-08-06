import { IconClose, IconPlus, IconTable } from '@/components/icons';
import { useConnections } from '@/state/connections';
import { useTabs } from '@/state/tabs';

export function TabBar() {
  const { tabs, activeId, setActive, closeTab, openQueryTab } = useTabs();
  const connections = useConnections((s) => s.items);
  const connected = connections.filter((c) => c.connected);

  return (
    <div
      className="flex h-9 shrink-0 items-stretch border-b"
      style={{ borderColor: 'var(--border)', background: 'var(--bg-subtle)' }}
    >
      <div className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
        {tabs.map((tab) => {
          const active = tab.id === activeId;
          const conn = connections.find((c) => c.id === tab.connectionId);
          return (
            <div
              key={tab.id}
              onClick={() => setActive(tab.id)}
              onAuxClick={(e) => {
                // Middle-click closes, as in browsers and editors.
                if (e.button === 1) closeTab(tab.id);
              }}
              className="group flex h-full min-w-0 max-w-52 cursor-pointer items-center gap-1.5 border-r px-2.5"
              style={{
                borderColor: 'var(--border)',
                background: active ? 'var(--bg)' : 'transparent',
                boxShadow: active ? 'inset 0 2px 0 var(--accent)' : undefined,
              }}
              title={conn ? `${tab.title} — ${conn.name}` : tab.title}
            >
              {tab.kind === 'table' && <IconTable size={11} className="shrink-0 opacity-55" />}
              {tab.running && (
                <span
                  className="h-1.5 w-1.5 shrink-0 animate-pulse rounded-full"
                  style={{ background: 'var(--accent)' }}
                />
              )}
              <span
                className="min-w-0 flex-1 truncate text-[12px]"
                style={{ color: active ? 'var(--text)' : 'var(--text-muted)' }}
              >
                {tab.title}
              </span>
              <button
                className="shrink-0 rounded p-0.5 opacity-0 transition-opacity group-hover:opacity-60 hover:!opacity-100"
                onClick={(e) => {
                  e.stopPropagation();
                  closeTab(tab.id);
                }}
                aria-label={`Close ${tab.title}`}
              >
                <IconClose size={11} />
              </button>
            </div>
          );
        })}

        <button
          className="btn btn-ghost shrink-0 px-2"
          onClick={() => openQueryTab(connected[0]?.id ?? null)}
          disabled={connected.length === 0}
          title={connected.length === 0 ? 'Connect to a database first' : 'New query tab'}
        >
          <IconPlus size={13} />
        </button>
      </div>
    </div>
  );
}
