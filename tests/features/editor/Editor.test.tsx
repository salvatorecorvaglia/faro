import { fireEvent, render } from '@testing-library/react';
import { createRef } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { Editor, type EditorHandle } from '@/features/editor/Editor';

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
    // real app running on macOS, exercised instead by "Shift-Enter" below,
    // which is platform-independent and bound to the same action.
    fireEvent.keyDown(host, { key: 'Enter', code: 'Enter', ctrlKey: true });
    expect(onRun).toHaveBeenCalledTimes(1);
  });

  it('runs on Shift-Enter too, as the same action', () => {
    const { host, onRun } = renderEditor();
    host.focus();
    fireEvent.keyDown(host, { key: 'Enter', code: 'Enter', shiftKey: true });
    expect(onRun).toHaveBeenCalledTimes(1);
  });

  it('exposes cursor, selection and replaceAll through the imperative handle', () => {
    const { handle, onChange } = renderEditor({ value: 'one two three' });

    expect(handle.current?.selection()).toBeNull();
    expect(handle.current?.cursor()).toBe(0);

    handle.current?.replaceAll('replaced');
    // replaceAll dispatches a real document change, which the update listener
    // reports back through onChange — the same path a keystroke takes.
    expect(onChange).toHaveBeenCalledWith('replaced');
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
