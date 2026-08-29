import { useRef, useState } from 'react';

import type { EditValue, Value } from '@/ipc/types';
import { formatValue, isNumeric } from '@/lib/value';
import { ROW_HEIGHT } from './gridLayout';

export function Cell({
  value,
  staged,
  width,
  selected,
  editing,
  editable,
  onClick,
  onStartEdit,
  onCommit,
  onCancel,
}: {
  value: Value | undefined;
  staged: EditValue | undefined;
  width: number;
  selected: boolean;
  editing: boolean;
  editable: boolean;
  onClick: () => void;
  onStartEdit: () => void;
  onCommit: (value: EditValue) => void;
  onCancel: () => void;
}) {
  // What is displayed: the staged edit if there is one, otherwise the stored
  // value. A staged cell is tinted so unsaved work is never mistaken for data
  // that is actually in the database.
  const dirty = staged !== undefined;
  const showsNull = dirty ? staged.kind === 'null' : !value || value.kind === 'null';
  const isDefault = dirty && staged.kind === 'default';

  const text = dirty
    ? staged.kind === 'text'
      ? staged.value
      : ''
    : value
      ? formatValue(value)
      : '';
  const numeric = !dirty && value ? isNumeric(value) : false;

  if (editing) {
    return <CellEditor width={width} initial={text} onCommit={onCommit} onCancel={onCancel} />;
  }

  const placeholder = isDefault ? 'default' : showsNull ? 'NULL' : text;

  return (
    <div
      role="gridcell"
      aria-selected={selected}
      className="selectable shrink-0 cursor-default truncate border-r px-2 leading-[26px]"
      style={{
        width,
        borderColor: 'var(--border)',
        textAlign: numeric ? 'right' : 'left',
        fontFamily: numeric || showsNull || isDefault ? 'var(--font-mono)' : undefined,
        // NULL is rendered as a dimmed italic marker so it cannot be confused
        // with an empty string or the literal text 'NULL'.
        color: showsNull || isDefault ? 'var(--null)' : undefined,
        fontStyle: showsNull || isDefault ? 'italic' : undefined,
        boxShadow: selected ? 'inset 0 0 0 2px var(--accent)' : undefined,
        background: dirty
          ? 'color-mix(in srgb, var(--warning) 22%, transparent)'
          : selected
            ? 'var(--accent-soft)'
            : undefined,
      }}
      onClick={onClick}
      onDoubleClick={() => editable && onStartEdit()}
      title={
        editable
          ? `${showsNull ? 'NULL' : text}\n\nDouble-click to edit`
          : showsNull
            ? 'NULL'
            : text
      }
    >
      {placeholder}
    </div>
  );
}

/**
 * Inline text entry for one cell.
 *
 * Enter commits, Escape abandons, and ⌘⌫ writes a real NULL — the grid has no
 * other way to express "no value" as distinct from the empty string, and
 * guessing from an emptied box would write the wrong one half the time.
 */
function CellEditor({
  width,
  initial,
  onCommit,
  onCancel,
}: {
  width: number;
  initial: string;
  onCommit: (value: EditValue) => void;
  onCancel: () => void;
}) {
  const [draft, setDraft] = useState(initial);

  // Enter/Escape/⌘⌫ all unmount this input once they resolve the edit, and
  // removing a focused element can still deliver a trailing native `blur` —
  // React's delegated `onBlur` would otherwise fire a second, redundant
  // commit, or (worse, after Escape) commit a value the user just cancelled.
  // This flag makes the first resolution final.
  const resolved = useRef(false);

  const commit = (value: EditValue) => {
    if (resolved.current) return;
    resolved.current = true;
    onCommit(value);
  };
  const cancel = () => {
    if (resolved.current) return;
    resolved.current = true;
    onCancel();
  };

  return (
    <input
      // This input is mounted only because the user just chose to edit this
      // cell, so focusing it is completing their action, not stealing focus.
      // biome-ignore lint/a11y/noAutofocus: see above
      autoFocus
      className="shrink-0 border-r px-2 text-[12px] leading-[26px] outline-none"
      style={{
        width,
        height: ROW_HEIGHT,
        borderColor: 'var(--accent)',
        background: 'var(--bg)',
        color: 'var(--text)',
        boxShadow: 'inset 0 0 0 2px var(--accent)',
        fontFamily: 'var(--font-mono)',
      }}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => commit({ kind: 'text', value: draft })}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          commit({ kind: 'text', value: draft });
        } else if (e.key === 'Escape') {
          e.preventDefault();
          cancel();
        } else if ((e.metaKey || e.ctrlKey) && e.key === 'Backspace') {
          e.preventDefault();
          commit({ kind: 'null' });
        }
      }}
      title="Enter to save · Esc to cancel · ⌘⌫ for NULL"
    />
  );
}
