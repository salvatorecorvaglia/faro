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
import { shortcut } from '@/lib/shortcuts';
import { useConnections } from '@/state/connections';
import { toCompletionSchema, useSchemaCache } from '@/state/schemaCache';
import { ROW_LIMIT_CHOICES, type Tab, useTabs } from '@/state/tabs';
import { useQueryExecution } from './useQueryExecution';

/**
 * Collapse a statement onto one line for the toolbar hint.
 *
 * A statement is usually several lines, and the hint is one — without this the
 * preview shows only the first line, which for a formatted query is often just
 * `SELECT`.
 */
function oneLine(sql: string): string {
  return sql.replace(/\s+/g, ' ').trim();
}

export function QueryTab({ tab }: { tab: Tab }) {
  const update = useTabs((s) => s.update);
  const connections = useConnections((s) => s.items);
  const connected = connections.filter((c) => c.connected);
  const conn = connections.find((c) => c.id === tab.connectionId);
  const engine = ((conn?.activeConfig ?? conn)?.engine as Engine | undefined) ?? null;
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

  const { run, runStatement, cursorStatement, setCursorOffset, tabRef } = useQueryExecution(
    tab,
    editor,
  );

  const format = useCallback(async () => {
    if (!canFormat) return;
    const t = tabRef.current;
    try {
      const { formatSql } = await import('@/lib/format');
      const current = useTabs.getState().tabs.find((candidate) => candidate.id === t.id);
      if (!current || current.sql !== t.sql || current.connectionId !== t.connectionId) return;
      const formatted = formatSql(t.sql, engine);
      if (formatted !== t.sql) editor.current?.replaceAll(formatted);
    } catch (error) {
      update(t.id, { error: ipc.errorMessage(error) });
    }
  }, [engine, canFormat, tabRef, update]);

  async function cancel() {
    if (!tab.connectionId || !tab.queryId) return;
    await ipc.cancelQuery(tab.connectionId, tab.queryId);
  }

  // ⌘↵ from anywhere in the tab, not just inside the editor.
  //
  // Events originating inside the editor are skipped: CodeMirror's own
  // `Mod-Enter` binding already handles those, and it calls `preventDefault`
  // without stopping propagation, so the keydown reaches this listener too.
  // Both handlers then ran in the same tick — two queries, two history rows,
  // and the second result overwriting the first.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (!(e.metaKey || e.ctrlKey)) return;

      // Cmd+S lives here rather than in `App` because this is where the tab's
      // SQL and its save dialog are. `App` had its own copy of both, so two
      // SaveQueryDialogs were mounted at once and the shortcut and the toolbar
      // button opened different ones. Only the active tab is mounted, so this
      // fires exactly when the old guard in `App` did.
      if (e.key.toLowerCase() === 's') {
        if (!tabRef.current.sql.trim()) return;
        e.preventDefault();
        setSaveOpen(true);
        return;
      }

      if (e.key !== 'Enter') return;
      const target = e.target;
      if (target instanceof Element && target.closest('.cm-editor')) return;
      e.preventDefault();
      // `runText` catches and surfaces its own errors on the tab.
      void (e.shiftKey ? runStatement() : run());
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [run, runStatement, tabRef]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-9 shrink-0 items-center gap-2 border-b px-2 border-line">
        {tab.running ? (
          <button type="button" className="btn btn-outline" onClick={cancel}>
            <IconStop size={11} /> Cancel
          </button>
        ) : (
          <button
            type="button"
            className="btn btn-primary"
            onClick={run}
            disabled={!tab.connectionId || !tab.sql.trim()}
            title={`Run — ${shortcut('Enter')} (runs the selection if there is one)`}
          >
            <IconPlay size={11} /> Run
          </button>
        )}

        {canFormat && (
          <button
            type="button"
            className="btn btn-ghost"
            onClick={format}
            disabled={!tab.sql.trim()}
            title={`Format SQL — ${shortcut('F', true)}`}
          >
            <IconWand size={12} /> Format
          </button>
        )}

        <button
          type="button"
          className="btn btn-ghost"
          onClick={() => setSaveOpen(true)}
          disabled={!tab.sql.trim()}
          title={`Save query — ${shortcut('S')}`}
        >
          <IconBookmark size={12} /> Save
        </button>

        <button
          type="button"
          className="btn btn-ghost"
          onClick={() => setExportOpen(true)}
          disabled={!exportable}
          title="Export these results"
        >
          <IconDownload size={12} /> Export
        </button>

        {tab.running && <Spinner size={12} />}

        {/* The statement under the cursor, and the one thing that runs just it.
            ⌘↵ deliberately still runs the whole script — a multi-statement
            script is a normal thing to want — so this is an addition rather
            than a redefinition of Run. */}
        <div className="min-w-0 flex-1 px-1">
          {!tab.running && cursorStatement && (
            <button
              type="button"
              className="block w-full truncate text-left font-mono text-2xs text-faint"
              onClick={runStatement}
              disabled={!tab.connectionId}
              title={`Run just this statement — ${shortcut('Enter', true)}\n\n${cursorStatement}`}
            >
              {shortcut('Enter', true)} {oneLine(cursorStatement)}
            </button>
          )}
        </div>

        {cached && cached.length > 0 && (
          <span className="text-2xs text-faint">{cached.length} tables indexed</span>
        )}

        {/* Every run was capped at the backend's default page and there was no
            way to ask for more, so the only recourse for a truncated result was
            to rewrite the query with a LIMIT of its own. */}
        <label className="flex items-center gap-1 text-2xs text-faint">
          Rows
          <select
            className="input w-auto py-1 text-xs"
            value={tab.rowLimit}
            onChange={(e) => update(tab.id, { rowLimit: Number(e.target.value) })}
            title="Most rows a run may return"
          >
            {ROW_LIMIT_CHOICES.map((n) => (
              <option key={n} value={n}>
                {n.toLocaleString()}
              </option>
            ))}
          </select>
        </label>

        <select
          className="input w-auto py-1 text-xs"
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
            onRunStatement={runStatement}
            onCursorChange={setCursorOffset}
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
        source={
          tab.resultQueryId
            ? { queryId: tab.resultQueryId, statement: tab.activeResultIndex }
            : null
        }
        connectionId={tab.connectionId}
        defaultName={tab.title}
      />
    </div>
  );
}
