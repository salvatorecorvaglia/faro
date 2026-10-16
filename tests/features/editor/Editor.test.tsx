import { MariaSQL, MSSQL, MySQL, PostgreSQL, SQLite } from '@codemirror/lang-sql';
import { fireEvent, render } from '@testing-library/react';
import { createRef } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { dialectFor, Editor, type EditorHandle } from '@/features/editor/Editor';

function renderEditor(props: Partial<React.ComponentProps<typeof Editor>> = {}) {
  const onChange = vi.fn();
  const onRun = vi.fn();
  const handle = createRef<EditorHandle | null>();
  const utils = render(
    <Editor
      value=""
      onChange={onChange}
      onRun={onRun}
      engine="postgres"
      schema={{}}
      handle={handle}
      {...props}
    />,
  );
  const host = utils.container.querySelector('.cm-content') as HTMLElement;
  return { ...utils, onChange, onRun, handle, host };
}

describe('Editor', () => {
  it('mounts a CodeMirror instance with the given text', () => {
    const { host } = renderEditor({ value: 'SELECT 1' });
    expect(host.textContent).toBe('SELECT 1');
  });

  it('runs on Mod-Enter', () => {
    const { host, onRun } = renderEditor();
    host.focus();
    // CodeMirror's "Mod" binds to whichever modifier the platform uses (Ctrl
    // here, since jsdom reports a non-Mac platform) — Cmd is covered by the
    // real app running on macOS.
    fireEvent.keyDown(host, { key: 'Enter', code: 'Enter', ctrlKey: true });
    expect(onRun).toHaveBeenCalledTimes(1);
  });

  it('does not run on Shift-Enter, which elsewhere types a newline', () => {
    // A Shift+Enter habit from chat apps used to run the whole script.
    const { host, onRun } = renderEditor();
    host.focus();
    fireEvent.keyDown(host, { key: 'Enter', code: 'Enter', shiftKey: true });
    expect(onRun).not.toHaveBeenCalled();
  });

  it('exposes selection and replaceAll through the imperative handle', () => {
    const { handle, onChange } = renderEditor({ value: 'one two three' });

    expect(handle.current?.selection()).toBeNull();

    handle.current?.replaceAll('replaced');
    // replaceAll dispatches a real document change, which the update listener
    // reports back through onChange — the same path a keystroke takes.
    expect(onChange).toHaveBeenCalledWith('replaced');
  });

  it('reads the cursor through the handle at the moment it is asked', () => {
    // "Run statement at cursor" resolves against this rather than against a
    // debounced copy, so it has to reflect the cursor as it is now.
    const { handle } = renderEditor({ value: 'SELECT 1' });
    expect(handle.current?.cursorOffset()).toBe(0);
  });

  it('highlights each engine with its own quoting rules', () => {
    // MySQL and MariaDB were highlighted as Postgres, where a backtick is not
    // a quote, and SQL Server's [brackets] likewise.
    expect(dialectFor('mysql')).toBe(MySQL);
    expect(dialectFor('mariadb')).toBe(MariaSQL);
    expect(dialectFor('sqlserver')).toBe(MSSQL);
    expect(dialectFor('sqlite')).toBe(SQLite);
    expect(dialectFor('postgres')).toBe(PostgreSQL);
    expect(dialectFor('clickhouse')).toBe(PostgreSQL);
  });

  it('syncs external value changes into the document without looping', () => {
    const { rerender, host, onChange } = renderEditor({ value: 'first' });
    expect(host.textContent).toBe('first');

    rerender(
      <Editor value="second" onChange={onChange} onRun={() => {}} engine="postgres" schema={{}} />,
    );
    expect(host.textContent).toBe('second');
    // The update listener reports every document change, including this
    // programmatic one, so onChange does fire once with the same text the
    // effect just wrote — that is an idempotent echo, not a loop. What the
    // "only when it differs" check actually prevents is re-dispatching (and
    // re-firing onChange) on every render that passes the same value again.
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith('second');

    onChange.mockClear();
    rerender(
      <Editor value="second" onChange={onChange} onRun={() => {}} engine="postgres" schema={{}} />,
    );
    expect(onChange).not.toHaveBeenCalled();
  });
});
