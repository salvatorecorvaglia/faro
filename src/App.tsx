import { useEffect, useState } from 'react';

import { IconDatabase, IconLighthouse } from '@/components/icons';
import { SplitGroup, SplitHandle, SplitPanel } from '@/components/panels';
import { UpdaterToast } from '@/components/UpdaterToast';
import { EmptyState } from '@/components/ui';
import { Sidebar } from '@/features/connections/Sidebar';
import { SaveQueryDialog } from '@/features/library/SaveQueryDialog';
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

export default function App() {
  useUpdater();

  const tabs = useTabs((s) => s.tabs);
  const activeId = useTabs((s) => s.activeId);
  const openQueryTab = useTabs((s) => s.openQueryTab);
  const closeTab = useTabs((s) => s.closeTab);
  const connections = useConnections((s) => s.items);
  const refreshHistory = useLibrary((s) => s.refreshHistory);

  const [paletteOpen, setPaletteOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);

  const active = tabs.find((t) => t.id === activeId);
  const connected = connections.filter((c) => c.connected);

  // Keep history fresh after a run finishes, so the panel and palette do not
  // lag behind what the user just did.
  const anyRunning = tabs.some((t) => t.running);
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
        case 's': {
          // Only meaningful for a query tab with something in it.
          const state = useTabs.getState();
          const current = state.tabs.find((t) => t.id === state.activeId);
          if (current?.kind === 'query' && current.sql.trim()) {
            e.preventDefault();
            setSaveOpen(true);
          }
          break;
        }
        case 't': {
          e.preventDefault();
          const state = useTabs.getState();
          // Inherit the current tab's connection so a new tab is immediately
          // usable rather than starting unattached.
          const current = state.tabs.find((t) => t.id === state.activeId);
          const id = current?.connectionId ?? connected[0]?.id ?? null;
          const engine = connections.find((c) => c.id === id)?.engine ?? null;
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
  }, [openQueryTab, closeTab, connected, connections]);

  return (
    <>
      <SplitGroup direction="horizontal" className="h-full">
        <SplitPanel defaultSize={20} minSize={12} maxSize={40}>
          <Sidebar />
        </SplitPanel>

        <SplitHandle direction="horizontal" />

        <SplitPanel>
          <div className="flex h-full flex-col" style={{ background: 'var(--bg)' }}>
            <TabBar />
            <div className="min-h-0 flex-1">
              {!active ? (
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
                        className="btn btn-primary"
                        onClick={() =>
                          openQueryTab(connected[0]!.id, starterQuery(connected[0]!.engine))
                        }
                      >
                        <IconDatabase size={13} /> New query
                      </button>
                    ) : undefined
                  }
                />
              ) : active.kind === 'query' ? (
                <QueryTab key={active.id} tab={active} />
              ) : (
                <TableTab key={active.id} tab={active} />
              )}
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

      <SaveQueryDialog
        open={saveOpen}
        onClose={() => setSaveOpen(false)}
        sql={active?.sql ?? ''}
        connectionId={active?.connectionId ?? null}
      />

      <UpdaterToast />
    </>
  );
}
