import { useShallow } from 'zustand/react/shallow';

import { IconClose, IconPlus, IconTable } from '@/components/icons';
import { rowActivation } from '@/components/ui';
import { useConnections } from '@/state/connections';
import { useTabs } from '@/state/tabs';

/**
 * One tab's button. Subscribes to just its own tab object, found by id.
 *
 * `update()` in the tabs store rebuilds the `tabs` array with `.map()`, which
 * keeps the same object reference for every tab it does not patch. So a
 * keystroke in one query tab's editor — which calls `update(tab.id, { sql })`
 * on every change — only ever changes *that* tab's reference. Subscribing
 * here with a plain selector (not `useShallow`) means Zustand's default
 * `Object.is` check does the right thing: this button re-renders only when
 * its own tab changes, not when the whole `tabs` array reference does.
 */
function TabBarItem({ id }: { id: string }) {
  const tab = useTabs((s) => s.tabs.find((t) => t.id === id));
  const activeId = useTabs((s) => s.activeId);
  const setActive = useTabs((s) => s.setActive);
  const closeTab = useTabs((s) => s.closeTab);
  const connectionName = useConnections(
    (s) => s.items.find((c) => c.id === tab?.connectionId)?.name,
  );
  if (!tab) return null;

  const active = tab.id === activeId;

  return (
    <div
      role="button"
      tabIndex={0}
      onKeyDown={rowActivation(() => setActive(tab.id)).onKeyDown}
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
      title={connectionName ? `${tab.title} — ${connectionName}` : tab.title}
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
        type="button"
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
}

export function TabBar() {
  // Only the id order, not the tab objects themselves — ids are primitives,
  // so `useShallow`'s elementwise comparison correctly ignores unrelated
  // field changes (like `sql`) inside tabs that did not move or get
  // added/removed. Each id's actual tab data is read by `TabBarItem`.
  const tabIds = useTabs(useShallow((s) => s.tabs.map((t) => t.id)));
  const openQueryTab = useTabs((s) => s.openQueryTab);
  const activeConnectionId = useTabs((s) => s.tabs.find((t) => t.id === s.activeId)?.connectionId);
  const connected = useConnections(
    useShallow((s) => s.items.filter((c) => c.connected).map((c) => c.id)),
  );

  return (
    <div
      className="flex h-9 shrink-0 items-stretch border-b"
      style={{ borderColor: 'var(--border)', background: 'var(--bg-subtle)' }}
    >
      <div className="flex min-w-0 flex-1 items-stretch overflow-x-auto">
        {tabIds.map((id) => (
          <TabBarItem key={id} id={id} />
        ))}

        <button
          type="button"
          className="btn btn-ghost shrink-0 px-2"
          onClick={() =>
            // Inherit the active tab's connection first, matching the ⌘T
            // shortcut (App.tsx) — the two are the same action and used to
            // pick different connections when the active tab wasn't
            // connected to the first item in the connected list.
            openQueryTab(activeConnectionId ?? connected[0] ?? null)
          }
          disabled={connected.length === 0}
          title={connected.length === 0 ? 'Connect to a database first' : 'New query tab'}
        >
          <IconPlus size={13} />
        </button>
      </div>
    </div>
  );
}
