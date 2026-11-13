import { type RefObject, useCallback, useEffect, useRef, useState } from 'react';
import type { EditorHandle } from '@/features/editor/Editor';
import * as ipc from '@/ipc';
import type { ScriptSummary } from '@/ipc/types';
import { shortcut } from '@/lib/shortcuts';
import { confirmDialog } from '@/state/confirm';
import { type Tab, useTabs } from '@/state/tabs';

let queryCounter = 0;

async function confirmScript(connectionId: string, sql: string): Promise<boolean> {
  let summary: ScriptSummary;
  try {
    summary = await ipc.scriptSummary(connectionId, sql);
  } catch {
    return true;
  }
  if (summary.statements < 2 || summary.writes === 0) return true;
  const writes = summary.writes === 1 ? '1 of them changes' : `${summary.writes} of them change`;
  return confirmDialog(
    `This runs all ${summary.statements} statements in the editor, and ${writes} data or schema.\n\nTo run only the statement under the cursor, press ${shortcut('Enter', true)} instead.`,
    { confirmLabel: 'Run all', danger: true },
  );
}

/** Coordinates execution, confirmation and cursor lookups independently of the toolbar. */
export function useQueryExecution(tab: Tab, editor: RefObject<EditorHandle | null>) {
  const update = useTabs((s) => s.update);
  // Read the latest tab inside async callbacks without making `run` depend on
  // every field of it, which would re-create the keybinding on each keystroke.
  const tabRef = useRef(tab);
  tabRef.current = tab;

  // Guards re-entry within a single tick. `t.running` comes from `tabRef`,
  // which is only reassigned during render, so two calls dispatched from the
  // same event both see `running: false` and both start a query.
  const inFlight = useRef(false);

  const runText = useCallback(
    async (sqlText: string, t = tabRef.current) => {
      const current = useTabs.getState().tabs.find((candidate) => candidate.id === t.id);
      if (
        !current ||
        current.running ||
        current.connectionId !== t.connectionId ||
        current.sql !== t.sql
      )
        return;
      if (!t.connectionId || t.running || inFlight.current) return;
      if (!sqlText.trim()) return;

      inFlight.current = true;
      const queryId = `q-${++queryCounter}`;
      update(t.id, { running: true, queryId, error: null, activeResultIndex: 0 });
      try {
        const result = await ipc.runQuery(t.connectionId, sqlText, queryId, t.rowLimit);
        update(t.id, {
          running: false,
          queryId: null,
          resultQueryId: queryId,
          results: result.statements,
          // Land on the failing statement: it is what the user needs to see.
          activeResultIndex: Math.max(
            0,
            result.statements.findIndex((s) => s.error),
          ),
        });
      } catch (e) {
        update(t.id, { running: false, queryId: null, error: ipc.errorMessage(e) });
      } finally {
        inFlight.current = false;
      }
    },
    [update],
  );

  // A run that is still being summarized or confirmed. `inFlight` is only set
  // once the query is sent, so without this a second click during the check
  // would queue a second confirmation for the same script.
  const checking = useRef(false);

  const run = useCallback(async () => {
    const t = tabRef.current;
    if (!t.connectionId || t.running || inFlight.current || checking.current) return;
    // The selection when there is one — the standard way to try part of a
    // script without deleting the rest — and otherwise the whole script.
    const text = editor.current?.selection() ?? t.sql;
    if (!text.trim()) return;
    checking.current = true;
    let proceed: boolean;
    try {
      proceed = await confirmScript(t.connectionId, text);
    } finally {
      checking.current = false;
    }
    if (proceed) return runText(text, t);
  }, [runText, editor]);

  /**
   * The statement the cursor is sitting in, resolved by the backend.
   *
   * Resolved there rather than here because the offset is a CodeMirror
   * position — a UTF-16 code unit index — and the splitter it has to agree with
   * is `sql::split_statements`. Re-implementing either in TypeScript is how the
   * two would come to disagree about where a statement ends.
   */
  const [cursorStatement, setCursorStatement] = useState<string | null>(null);
  const [cursorOffset, setCursorOffset] = useState(0);
  // The same offset without waiting for a render, for `runStatement`'s
  // fallback when the editor handle is not mounted.
  const cursorOffsetRef = useRef(0);
  cursorOffsetRef.current = cursorOffset;

  useEffect(() => {
    if (!tab.sql.trim()) {
      setCursorStatement(null);
      return;
    }
    // Debounced: the cursor moves on every keystroke and arrow key, and this is
    // a round trip for a toolbar hint, not for anything the user is waiting on.
    let cancelled = false;
    const timer = setTimeout(() => {
      ipc
        .statementAtCursor(tab.sql, cursorOffset)
        .then((stmt) => {
          if (!cancelled) setCursorStatement(stmt);
        })
        .catch(() => {
          if (!cancelled) setCursorStatement(null);
        });
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [tab.sql, cursorOffset]);

  /**
   * Run the statement under the cursor *now*, not the one the hint last saw.
   *
   * `cursorStatement` is debounced for the toolbar, so for 120ms after a
   * cursor move or a keystroke it still names the previous statement — and
   * running it then executed a statement the cursor had already left, which in
   * a script mixing SELECT and DELETE is the wrong kind of surprise. The
   * resolution is repeated here against the live document and cursor.
   */
  const runStatement = useCallback(async () => {
    const t = tabRef.current;
    const offset = editor.current?.cursorOffset() ?? cursorOffsetRef.current;
    let statement: string | null;
    try {
      statement = await ipc.statementAtCursor(t.sql, offset);
    } catch (e) {
      update(t.id, { error: ipc.errorMessage(e) });
      return;
    }
    if (statement) return runText(statement, t);
  }, [runText, update, editor]);

  return { run, runStatement, cursorStatement, setCursorOffset, tabRef };
}
