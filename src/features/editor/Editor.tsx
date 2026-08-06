import { autocompletion, closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { json } from '@codemirror/lang-json';
import { PostgreSQL, SQLite, sql } from '@codemirror/lang-sql';
import { bracketMatching, HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import { Compartment, EditorState, type Extension } from '@codemirror/state';
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
  placeholder,
} from '@codemirror/view';
import { tags as t } from '@lezer/highlight';
import { useEffect, useRef } from 'react';

import type { Engine } from '@/ipc/types';
import { editorPlaceholder, isSqlEngine } from '@/lib/engine';

export interface EditorHandle {
  /** Text the user selected, or null when the selection is empty. */
  selection: () => string | null;
  /** Cursor offset, for resolving the statement under it. */
  cursor: () => number;
  replaceAll: (text: string) => void;
}

/**
 * The SQL editor.
 *
 * CodeMirror is created imperatively once and kept in a ref: rebuilding the
 * view on every React render would drop cursor position and undo history.
 * Document changes flow out through `onChange`; changes flowing *in* are only
 * applied when they differ from what the editor already holds, which prevents
 * a feedback loop.
 *
 * The dialect and the autocomplete schema live in Compartments so they can be
 * reconfigured — switching connection, or the schema cache arriving — without
 * tearing down the editor and losing the user's undo history.
 */
export function Editor({
  value,
  onChange,
  onRun,
  onFormat,
  engine,
  schema,
  handle,
  placeholderText,
}: {
  value: string;
  onChange: (v: string) => void;
  onRun: () => void;
  onFormat?: () => void;
  engine: Engine | null;
  /** Table name → column names, for autocomplete. */
  schema: Record<string, string[]>;
  handle?: React.RefObject<EditorHandle | null>;
  placeholderText?: string;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const sqlConf = useRef(new Compartment());

  // Keep the latest callbacks in refs so keymap closures never go stale
  // without needing to rebuild the editor.
  const runRef = useRef(onRun);
  const changeRef = useRef(onChange);
  const formatRef = useRef(onFormat);
  runRef.current = onRun;
  changeRef.current = onChange;
  formatRef.current = onFormat;

  useEffect(() => {
    if (!host.current) return;

    const runBinding = {
      preventDefault: true,
      run: () => {
        runRef.current();
        return true;
      },
    };

    const extensions: Extension[] = [
      lineNumbers(),
      highlightActiveLine(),
      highlightActiveLineGutter(),
      history(),
      drawSelection(),
      bracketMatching(),
      closeBrackets(),
      autocompletion({
        // The editor is for writing SQL, so completions should feel eager —
        // but not so eager that they fire on the first character of a word.
        activateOnTyping: true,
        maxRenderedOptions: 30,
        icons: false,
      }),
      sqlConf.current.of(languageExtension(engine, schema, placeholderText)),
      syntaxHighlighting(highlightStyle),
      keymap.of([
        { key: 'Mod-Enter', ...runBinding },
        // Shift-Enter is the same action; both are common muscle memory.
        { key: 'Shift-Enter', ...runBinding },
        {
          key: 'Mod-Shift-f',
          preventDefault: true,
          run: () => {
            formatRef.current?.();
            return true;
          },
        },
        ...closeBracketsKeymap,
        ...historyKeymap,
        ...defaultKeymap,
        indentWithTab,
      ]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged) changeRef.current(u.state.doc.toString());
      }),
      EditorView.lineWrapping,
      theme,
    ];

    const view_ = new EditorView({
      state: EditorState.create({ doc: value, extensions }),
      parent: host.current,
    });
    view.current = view_;
    view_.focus();

    return () => {
      view_.destroy();
      view.current = null;
    };
    // Built once. Dialect and schema are swapped via the compartment below,
    // and `value` is synced by its own effect, so typing never rebuilds.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Reconfigure the SQL dialect and completion schema in place.
  useEffect(() => {
    view.current?.dispatch({
      effects: sqlConf.current.reconfigure(languageExtension(engine, schema, placeholderText)),
    });
  }, [engine, schema, placeholderText]);

  useEffect(() => {
    const v = view.current;
    if (!v) return;
    const current = v.state.doc.toString();
    if (current === value) return;
    v.dispatch({ changes: { from: 0, to: current.length, insert: value } });
  }, [value]);

  useEffect(() => {
    if (!handle) return;
    handle.current = {
      selection: () => {
        const v = view.current;
        if (!v) return null;
        const { from, to } = v.state.selection.main;
        return from === to ? null : v.state.sliceDoc(from, to);
      },
      cursor: () => view.current?.state.selection.main.head ?? 0,
      replaceAll: (text) => {
        const v = view.current;
        if (!v) return;
        v.dispatch({
          changes: { from: 0, to: v.state.doc.length, insert: text },
        });
      },
    };
  }, [handle]);

  return <div ref={host} className="h-full overflow-auto text-[12.5px]" />;
}

/**
 * The language extension for an engine.
 *
 * MongoDB queries are documents, so they get JSON highlighting and bracket
 * matching. Loading the SQL mode there would colour `find` as a keyword and
 * offer table names as SQL completions, both of which would be misleading.
 */
function languageExtension(
  engine: Engine | null,
  schema: Record<string, string[]>,
  placeholderText: string | undefined,
): Extension {
  // The placeholder lives in the same compartment so switching a tab's
  // connection updates it; built once at mount it would go stale.
  const hint = placeholder(placeholderText ?? editorPlaceholder(engine));
  if (!isSqlEngine(engine)) return [json(), hint];
  return [sql({ dialect: dialectFor(engine), schema, upperCaseKeywords: false }), hint];
}

function dialectFor(engine: Engine | null) {
  switch (engine) {
    case 'sqlite':
    case 'duckdb':
      return SQLite;
    // Postgres is the closest match for the PG-wire family and a reasonable
    // default for engines whose dialect is not yet modelled.
    default:
      return PostgreSQL;
  }
}

/**
 * Syntax colours, drawn from the app's CSS variables where a token maps onto
 * an existing role, and from a small set of literals otherwise. Keeping them
 * here rather than in styles.css means one place to look when a token reads
 * wrong.
 */
const highlightStyle = HighlightStyle.define([
  { tag: t.keyword, color: 'var(--syntax-keyword)', fontWeight: '600' },
  { tag: [t.string, t.special(t.string)], color: 'var(--syntax-string)' },
  { tag: [t.number, t.bool, t.null], color: 'var(--syntax-number)' },
  { tag: t.comment, color: 'var(--syntax-comment)', fontStyle: 'italic' },
  {
    tag: [t.function(t.variableName), t.standard(t.variableName)],
    color: 'var(--syntax-function)',
  },
  { tag: [t.typeName, t.className], color: 'var(--syntax-type)' },
  { tag: t.operator, color: 'var(--syntax-operator)' },
  { tag: t.punctuation, color: 'var(--text-muted)' },
]);

/**
 * Editor theme.
 *
 * Colours come from the same CSS variables as the rest of the app, so the
 * editor follows the light/dark switch without a second palette to maintain.
 */
const theme = EditorView.theme({
  '&': {
    height: '100%',
    backgroundColor: 'var(--bg)',
    color: 'var(--text)',
    fontFamily: 'var(--font-mono)',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.6' },
  '.cm-content': { padding: '8px 0', caretColor: 'var(--accent)' },
  '.cm-gutters': {
    backgroundColor: 'var(--bg)',
    color: 'var(--text-faint)',
    border: 'none',
    paddingRight: '4px',
  },
  '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--accent) 6%, transparent)' },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text-muted)' },
  '.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection': {
    backgroundColor: 'var(--selection)',
  },
  '.cm-cursor': { borderLeftColor: 'var(--accent)', borderLeftWidth: '2px' },
  '.cm-placeholder': { color: 'var(--text-faint)' },
  '.cm-matchingBracket, &.cm-focused .cm-matchingBracket': {
    backgroundColor: 'color-mix(in srgb, var(--accent) 22%, transparent)',
    outline: 'none',
  },
  // Completion popup, styled to match the app rather than CodeMirror's default.
  '.cm-tooltip.cm-tooltip-autocomplete': {
    background: 'var(--bg-subtle)',
    border: '1px solid var(--border-strong)',
    borderRadius: '6px',
    boxShadow: '0 8px 24px rgb(0 0 0 / 0.18)',
    overflow: 'hidden',
  },
  '.cm-tooltip-autocomplete > ul': {
    fontFamily: 'var(--font-mono)',
    fontSize: '12px',
    maxHeight: '16em',
  },
  '.cm-tooltip-autocomplete > ul > li': { padding: '3px 8px' },
  '.cm-tooltip-autocomplete > ul > li[aria-selected]': {
    background: 'var(--accent)',
    color: '#fff',
  },
  '.cm-completionLabel': { color: 'inherit' },
  '.cm-completionDetail': {
    color: 'var(--text-faint)',
    fontStyle: 'normal',
    marginLeft: '8px',
  },
  'li[aria-selected] .cm-completionDetail': { color: 'rgb(255 255 255 / 0.75)' },
});
