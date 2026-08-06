import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { IconBookmark, IconDownload, IconPlay, IconStop, IconWand } from '@/components/icons';
import { SplitGroup, SplitHandle, SplitPanel } from '@/components/panels';
import { Spinner } from '@/components/ui';
import { Editor, type EditorHandle } from '@/features/editor/Editor';
import { SaveQueryDialog } from '@/features/library/SaveQueryDialog';
import { ResultPanel } from '@/features/results/ResultPanel';
import { ExportDialog } from '@/features/transfer/ExportDialog';
import * as ipc from '@/ipc';
import type { Engine } from '@/ipc/types';
import { isSqlEngine } from '@/lib/engine';
import { formatSql } from '@/lib/format';
import { useConnections } from '@/state/connections';
import { toCompletionSchema, useSchemaCache } from '@/state/schemaCache';
import { type Tab, useTabs } from '@/state/tabs';

let queryCounter = 0;

export function QueryTab({ tab }: { tab: Tab }) {
  const update = useTabs((s) => s.update);
  const connections = useConnections((s) => s.items);
  const connected = connections.filter((c) => c.connected);
  const conn = connections.find((c) => c.id === tab.connectionId);
  const engine = (conn?.engine as Engine | undefined) ?? null;
  // The SQL formatter would mangle a Mongo query document, so it is hidden
  // rather than offered and silently doing nothing useful.
  const canFormat = isSqlEngine(engine);

  const editor = useRef<EditorHandle | null>(null);
  const [saveOpen, setSaveOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);

  // Only the statement currently on screen can be exported.
  const shownResult = (() => {
    const outcome = tab.results[tab.activeResultIndex]?.outcome;
    return outcome?.type === 'rows' ? outcome : null;
  })();
  const exportable = !!shownResult;

  const ensureSchema = useSchemaCache((s) => s.ensure);
  const cached = useSchemaCache((s) =>
    tab.connectionId ? s.byConnection[tab.connectionId] : undefined,
  );

  useEffect(() => {
    if (tab.connectionId) ensureSchema(tab.connectionId);
  }, [tab.connectionId, ensureSchema]);

  // Memoized so the editor's reconfigure effect does not fire on every render;
  // a new object identity each time would rebuild the completion source.
  const completionSchema = useMemo(() => toCompletionSchema(cached ?? []), [cached]);

  // Read the latest tab inside async callbacks without making `run` depend on
  // every field of it, which would re-create the keybinding on each keystroke.
  const tabRef = useRef(tab);
  tabRef.current = tab;

  const run = useCallback(async () => {
    const t = tabRef.current;
    if (!t.connectionId || t.running) return;

    // Run the selection when there is one — the standard way to try part of a
    // script without deleting the rest.
    const selected = editor.current?.selection();
    const sqlText = selected ?? t.sql;
    if (!sqlText.trim()) return;

    const queryId = `q-${++queryCounter}`;
    update(t.id, { running: true, queryId, error: null, activeResultIndex: 0 });
    try {
      const result = await ipc.runQuery(t.connectionId, sqlText, queryId);
      update(t.id, {
        running: false,
        queryId: null,
        results: result.statements,
        // Land on the failing statement: it is what the user needs to see.
        activeResultIndex: Math.max(
          0,
          result.statements.findIndex((s) => s.error),
        ),
      });
    } catch (e) {
      update(t.id, { running: false, queryId: null, error: ipc.errorMessage(e) });
    }
  }, [update]);

  const format = useCallback(() => {
    if (!canFormat) return;
    const t = tabRef.current;
    const formatted = formatSql(t.sql, engine);
    if (formatted !== t.sql) editor.current?.replaceAll(formatted);
  }, [engine, canFormat]);

  async function cancel() {
    if (!tab.connectionId || !tab.queryId) return;
    await ipc.cancelQuery(tab.connectionId, tab.queryId);
  }

  // ⌘↵ from anywhere in the tab, not just inside the editor.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        run();
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [run]);

  return (
    <div className="flex h-full flex-col">
      <div
        className="flex h-9 shrink-0 items-center gap-2 border-b px-2"
        style={{ borderColor: 'var(--border)' }}
      >
        {tab.running ? (
          <button className="btn btn-outline" onClick={cancel}>
            <IconStop size={11} /> Cancel
          </button>
        ) : (
          <button
            className="btn btn-primary"
            onClick={run}
            disabled={!tab.connectionId || !tab.sql.trim()}
            title="Run — ⌘↵ (runs the selection if there is one)"
          >
            <IconPlay size={11} /> Run
          </button>
        )}

        {canFormat && (
          <button
            className="btn btn-ghost"
            onClick={format}
            disabled={!tab.sql.trim()}
            title="Format SQL — ⌘⇧F"
          >
            <IconWand size={12} /> Format
          </button>
        )}

        <button
          className="btn btn-ghost"
          onClick={() => setSaveOpen(true)}
          disabled={!tab.sql.trim()}
          title="Save query — ⌘S"
        >
          <IconBookmark size={12} /> Save
        </button>

        <button
          className="btn btn-ghost"
          onClick={() => setExportOpen(true)}
          disabled={!exportable}
          title="Export these results"
        >
          <IconDownload size={12} /> Export
        </button>

        {tab.running && <Spinner size={12} />}

        <div className="flex-1" />

        {cached && cached.length > 0 && (
          <span className="text-[10.5px]" style={{ color: 'var(--text-faint)' }}>
            {cached.length} tables indexed
          </span>
        )}

        <select
          className="input w-auto py-1 text-[11.5px]"
          value={tab.connectionId ?? ''}
          onChange={(e) => update(tab.id, { connectionId: e.target.value || null })}
        >
          <option value="">No connection</option>
          {connected.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
      </div>

      <SplitGroup direction="vertical" className="min-h-0 flex-1">
        <SplitPanel defaultSize={45} minSize={15}>
          <Editor
            value={tab.sql}
            onChange={(sql) => update(tab.id, { sql })}
            onRun={run}
            onFormat={format}
            engine={engine}
            schema={completionSchema}
            handle={editor}
          />
        </SplitPanel>

        <SplitHandle direction="vertical" />

        <SplitPanel defaultSize={55} minSize={15}>
          <ResultPanel
            statements={tab.results}
            browseResult={null}
            error={tab.error}
            activeIndex={tab.activeResultIndex}
            onActiveIndexChange={(i) => update(tab.id, { activeResultIndex: i })}
            running={tab.running}
            // Query results are already fetched, so sorting and filtering
            // happen in the client over the page in hand.
            clientSideSort
          />
        </SplitPanel>
      </SplitGroup>

      <SaveQueryDialog
        open={saveOpen}
        onClose={() => setSaveOpen(false)}
        sql={tab.sql}
        connectionId={tab.connectionId}
      />

      <ExportDialog
        open={exportOpen}
        onClose={() => setExportOpen(false)}
        result={shownResult}
        connectionId={tab.connectionId}
        defaultName={tab.title}
      />
    </div>
  );
}
