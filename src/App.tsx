import { useEffect, useState } from 'react';

import { ConfirmHost } from '@/components/ConfirmDialog';
import { IconDatabase, IconLighthouse } from '@/components/icons';
import { SplitGroup, SplitHandle, SplitPanel } from '@/components/panels';
import { UpdaterToast } from '@/components/UpdaterToast';
import { EmptyState } from '@/components/ui';
import { Sidebar } from '@/features/connections/Sidebar';
import { CommandPalette } from '@/features/palette/CommandPalette';
import { ShortcutSheet } from '@/features/palette/ShortcutSheet';
import { QueryTab } from '@/features/tabs/QueryTab';
import { TabBar } from '@/features/tabs/TabBar';
import { TableTab } from '@/features/tabs/TableTab';
import { useUpdater } from '@/hooks/useUpdater';
import { starterQuery } from '@/lib/engine';
import { useConnections } from '@/state/connections';
import { useLibrary } from '@/state/library';
import { useTabs } from '@/state/tabs';

/**
 * Whatever fills the main area: the active tab, or the empty state.
 *
 * Its own component so that it — and only it — subscribes to the active tab.
 * Every keystroke in the editor writes the tab's SQL into the store, which
 * produces a new tab object; subscribed from `App`, that re-rendered the whole
 * shell on each key — sidebar, schema tree, palette and all — for a change
 * only the editor needed to see.
 */
function ActiveTab() {
  const active = useTabs((s) => s.tabs.find((t) => t.id === s.activeId));
  const openQueryTab = useTabs((s) => s.openQueryTab);
  const connections = useConnections((s) => s.items);

  if (active) {
    return active.kind === 'query' ? (
      <QueryTab key={active.id} tab={active} />
    ) : (
      <TableTab key={active.id} tab={active} />
    );
  }

  const connected = connections.filter((c) => c.connected);
  return (
    <EmptyState
      icon={<IconLighthouse size={40} />}
      title={connected.length === 0 ? 'Connect to a database' : 'No tab open'}
      hint={
        connected.length === 0
          ? 'Add a connection in the sidebar to browse tables and run queries.'
          : 'Press ⌘K to search, ⌘T for a new query tab, or ? for keyboard shortcuts.'
      }
      action={
        connected.length > 0 ? (
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => openQueryTab(connected[0]!.id, starterQuery(connected[0]!.engine))}
          >
            <IconDatabase size={13} /> New query
          </button>
        ) : undefined
      }
    />
  );
}

export default function App() {
  useUpdater();

  const openQueryTab = useTabs((s) => s.openQueryTab);
  const closeTab = useTabs((s) => s.closeTab);
  const refreshHistory = useLibrary((s) => s.refreshHistory);

  const [paletteOpen, setPaletteOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);

  // Keep history fresh after a run finishes, so the panel and palette do not
  // lag behind what the user just did. A boolean selector, so the shell
  // re-renders when a run starts or ends and not on every edit.
  const anyRunning = useTabs((s) => s.tabs.some((t) => t.running));
  useEffect(() => {
    if (!anyRunning) refreshHistory();
  }, [anyRunning, refreshHistory]);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      // Never steal a key from a field the user is typing into — the editor and
      // every filter box would otherwise lose '?'.
      const typing =
        !!target &&
        (target.isContentEditable ||
          ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName) ||
          !!target.closest('.cm-editor'));

      if (e.key === '?' && !typing) {
        e.preventDefault();
        setShortcutsOpen((v) => !v);
        return;
      }

      const mod = e.metaKey || e.ctrlKey;
      if (!mod) return;

      switch (e.key.toLowerCase()) {
        case 'k':
          e.preventDefault();
          setPaletteOpen((v) => !v);
          break;
        case 't': {
          e.preventDefault();
          const state = useTabs.getState();
          // Inherit the current tab's connection so a new tab is immediately
          // usable rather than starting unattached.
          const current = state.tabs.find((t) => t.id === state.activeId);
          // Read connections from the store rather than closing over them:
          // `connected` is a fresh array each render, so depending on it
          // detached and reattached this listener on every keystroke.
          const all = useConnections.getState().items;
          const id = current?.connectionId ?? all.find((c) => c.connected)?.id ?? null;
          const engine = all.find((c) => c.id === id)?.engine ?? null;
          openQueryTab(id, starterQuery(engine));
          break;
        }
        case 'w': {
          e.preventDefault();
          const id = useTabs.getState().activeId;
          if (id) closeTab(id);
          break;
        }
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // Both deps are stable store actions, so the listener is attached once.
  }, [openQueryTab, closeTab]);

  return (
    <>
      <SplitGroup direction="horizontal" className="h-full">
        <SplitPanel defaultSize={20} minSize={12} maxSize={40}>
          <Sidebar />
        </SplitPanel>

        <SplitHandle direction="horizontal" />

        <SplitPanel>
          <div className="flex h-full flex-col bg-surface">
            <TabBar />
            <div className="min-h-0 flex-1">
              <ActiveTab />
            </div>
          </div>
        </SplitPanel>
      </SplitGroup>

      <CommandPalette
        open={paletteOpen}
        onClose={() => setPaletteOpen(false)}
        onShowShortcuts={() => setShortcutsOpen(true)}
      />

      <ShortcutSheet open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />

      <UpdaterToast />
      <ConfirmHost />
    </>
  );
}
