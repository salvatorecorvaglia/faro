import { useState } from 'react';

import { IconClose, IconWarning } from '@/components/icons';
import { ErrorBanner, Spinner } from '@/components/ui';
import * as ipc from '@/ipc';
import type { GuardedStatement } from '@/ipc/types';
import { confirmDialog } from '@/state/confirm';

/**
 * The bar that appears once there are unsaved changes.
 *
 * Nothing is written until Apply is pressed, and the SQL can be inspected
 * first. Someone about to modify production data should be able to read the
 * exact statements — and the preview comes from the same backend code path
 * that executes them, so it cannot drift from what actually runs.
 */
export function ApplyBar({
  changeCount,
  applying,
  error,
  onApply,
  onDiscard,
  onPreview,
  onDismissError,
}: {
  changeCount: number;
  applying: boolean;
  error: string | null;
  onApply: () => void;
  onDiscard: () => void;
  onPreview: () => Promise<GuardedStatement[]>;
  onDismissError: () => void;
}) {
  const [preview, setPreview] = useState<GuardedStatement[] | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);

  async function togglePreview() {
    if (preview) {
      setPreview(null);
      return;
    }
    setPreviewError(null);
    try {
      setPreview(await onPreview());
    } catch (e) {
      // `ipc.errorMessage`, not `String(e)`: Tauri rejects with the serialized
      // `FaroError` object, and stringifying that renders "[object Object]".
      setPreviewError(ipc.errorMessage(e));
    }
  }

  // Applying writes to the database and cannot be undone, and the SQL preview
  // that would show what is about to run is opt-in. Every other destructive
  // action in the app confirms first; this one did not.
  async function confirmAndApply() {
    const summary = `Apply ${changeCount} ${changeCount === 1 ? 'change' : 'changes'} to the database?`;
    const proceed = await confirmDialog(`${summary}\n\nThis cannot be undone.`, {
      confirmLabel: 'Apply',
      danger: true,
    });
    if (proceed) onApply();
  }

  return (
    <div
      className="shrink-0 border-t"
      style={{
        borderColor: 'var(--warning)',
        background: 'color-mix(in srgb, var(--warning) 10%, var(--bg))',
      }}
    >
      <div className="flex h-9 items-center gap-2 px-2.5">
        <IconWarning size={12} style={{ color: 'var(--warning)' }} />
        <span className="text-[12px] font-medium">
          {changeCount} unsaved {changeCount === 1 ? 'change' : 'changes'}
        </span>

        <button
          type="button"
          className="btn btn-ghost text-[11px]"
          onClick={togglePreview}
          disabled={applying}
        >
          {preview ? 'Hide SQL' : 'Show SQL'}
        </button>

        <div className="flex-1" />

        <button type="button" className="btn btn-ghost" onClick={onDiscard} disabled={applying}>
          Discard
        </button>
        <button
          type="button"
          className="btn btn-primary"
          onClick={confirmAndApply}
          disabled={applying}
        >
          {applying && <Spinner size={11} />}
          Apply
        </button>
      </div>

      {previewError && (
        <div className="px-2.5 pb-2">
          <ErrorBanner message={previewError} onDismiss={() => setPreviewError(null)} />
        </div>
      )}

      {preview && (
        <div
          className="max-h-40 overflow-auto border-t px-2.5 py-2"
          style={{ borderColor: 'var(--border)' }}
        >
          {preview.length === 0 ? (
            <p className="text-[11px]" style={{ color: 'var(--text-faint)' }}>
              Nothing to run.
            </p>
          ) : (
            <div className="flex flex-col gap-1">
              {preview.map((s, i) => (
                <div key={i} className="flex items-start gap-2">
                  <span
                    className="shrink-0 pt-0.5 text-[10px] tabular-nums"
                    style={{ color: 'var(--text-faint)' }}
                  >
                    {i + 1}
                  </span>
                  <code
                    className="selectable min-w-0 flex-1 whitespace-pre-wrap break-all font-mono text-[11px]"
                    style={{ color: 'var(--text)' }}
                  >
                    {s.sql}
                  </code>
                  {s.expect !== null && (
                    <span
                      className="shrink-0 pt-0.5 text-[10px]"
                      style={{ color: 'var(--text-faint)' }}
                      title="This statement must affect exactly this many rows, or the whole batch is rolled back"
                    >
                      = {s.expect} row
                    </span>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {error && (
        <div className="px-2.5 pb-2">
          <ErrorBanner message={error} onDismiss={onDismissError} />
        </div>
      )}
    </div>
  );
}

/** Explains why a table cannot be edited, shown in place of the apply bar. */
export function ReadOnlyNotice({ reason, onDismiss }: { reason: string; onDismiss: () => void }) {
  return (
    <div
      className="flex shrink-0 items-center gap-2 border-t px-2.5 py-1.5"
      style={{ borderColor: 'var(--border)', background: 'var(--bg-subtle)' }}
    >
      <IconWarning size={11} style={{ color: 'var(--text-faint)' }} />
      <span className="flex-1 text-[11px]" style={{ color: 'var(--text-muted)' }}>
        {reason}
      </span>
      <button
        type="button"
        className="opacity-60 hover:opacity-100"
        onClick={onDismiss}
        aria-label="Dismiss"
      >
        <IconClose size={11} />
      </button>
    </div>
  );
}
