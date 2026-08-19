import { Modal } from '@/components/ui';

/**
 * The keyboard reference, opened with `?` or from the palette.
 *
 * Modifiers are rendered per platform: showing `⌘` to a Windows user is worse
 * than showing nothing, because they will try it.
 */
const IS_APPLE = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform);

const MOD = IS_APPLE ? '⌘' : 'Ctrl';
const SHIFT = IS_APPLE ? '⇧' : 'Shift';
const ENTER = IS_APPLE ? '↵' : 'Enter';
const DELETE = IS_APPLE ? '⌫' : 'Backspace';

interface Shortcut {
  keys: string;
  action: string;
  note?: string;
}

const GROUPS: { title: string; shortcuts: Shortcut[] }[] = [
  {
    title: 'Everywhere',
    shortcuts: [
      {
        keys: `${MOD} K`,
        action: 'Command palette',
        note: 'queries, history, tables, connections',
      },
      { keys: '?', action: 'This list' },
      { keys: `${MOD} T`, action: 'New query tab' },
      { keys: `${MOD} W`, action: 'Close tab' },
    ],
  },
  {
    title: 'Query editor',
    shortcuts: [
      { keys: `${MOD} ${ENTER}`, action: 'Run', note: 'the selection, if there is one' },
      { keys: `${SHIFT} ${ENTER}`, action: 'Run', note: 'the same thing' },
      { keys: `${MOD} ${SHIFT} F`, action: 'Format SQL', note: 'SQL engines only' },
      { keys: `${MOD} S`, action: 'Save the query' },
      { keys: `${MOD} Z`, action: 'Undo' },
    ],
  },
  {
    title: 'Result grid',
    shortcuts: [
      { keys: 'Click a header', action: 'Sort', note: 'ascending, descending, off' },
      { keys: 'Drag a header', action: 'Reorder columns' },
      { keys: 'Drag its edge', action: 'Resize a column' },
      { keys: 'Double-click a cell', action: 'Edit', note: 'tables with a primary key' },
      { keys: '↑ ↓ ← →', action: 'Move the selected cell' },
      { keys: 'Home / End', action: 'Jump to the first / last column' },
      { keys: 'Page Up / Page Down', action: 'Move 20 rows' },
      { keys: ENTER, action: 'Open the cell inspector', note: 'when a cell is selected' },
      { keys: `${MOD} ${DELETE}`, action: 'Set a cell to NULL', note: 'while editing it' },
      { keys: ENTER, action: 'Commit the cell', note: 'while editing it' },
      { keys: 'Esc', action: 'Abandon the cell', note: 'while editing it' },
    ],
  },
];

export function ShortcutSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <Modal open={open} onClose={onClose} title="Keyboard shortcuts" width={520}>
      <div className="flex flex-col gap-4">
        {GROUPS.map((group) => (
          <div key={group.title}>
            <h3
              className="mb-1.5 text-[10.5px] font-semibold uppercase tracking-wide"
              style={{ color: 'var(--text-faint)' }}
            >
              {group.title}
            </h3>
            <div className="flex flex-col gap-0.5">
              {group.shortcuts.map((s) => (
                <div key={s.keys + s.action} className="flex items-baseline gap-2 text-[12px]">
                  <kbd
                    className="shrink-0 rounded px-1.5 py-0.5 font-mono text-[11px]"
                    style={{
                      background: 'var(--bg-inset)',
                      color: 'var(--text)',
                      minWidth: 96,
                      textAlign: 'center',
                    }}
                  >
                    {s.keys}
                  </kbd>
                  <span>{s.action}</span>
                  {s.note && (
                    <span className="text-[11px]" style={{ color: 'var(--text-faint)' }}>
                      — {s.note}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </div>
        ))}

        <p className="text-[11px]" style={{ color: 'var(--text-faint)' }}>
          Results are capped at 1 000 rows per page. When more exist, the footer says so rather than
          letting a partial result look complete.
        </p>
      </div>
    </Modal>
  );
}
