import { useEffect, useState } from 'react';

import { ErrorBanner, Field, Modal } from '@/components/ui';
import * as ipc from '@/ipc';
import type { SavedQuery } from '@/ipc/types';
import { folderNames, useLibrary } from '@/state/library';

/**
 * Save the current editor contents, or rename an existing saved query.
 *
 * `editing` distinguishes the two: when set, the dialog updates that row
 * instead of creating another one.
 */
export function SaveQueryDialog({
  open,
  onClose,
  sql,
  connectionId,
  editing,
}: {
  open: boolean;
  onClose: () => void;
  sql: string;
  connectionId: string | null;
  editing?: SavedQuery | null;
}) {
  const save = useLibrary((s) => s.save);
  const saved = useLibrary((s) => s.saved);

  const [name, setName] = useState('');
  const [folder, setFolder] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setName(editing?.name ?? '');
    setFolder(editing?.folder ?? '');
    setError(null);
  }, [open, editing]);

  const existingFolders = folderNames(saved);

  async function onSave() {
    setBusy(true);
    setError(null);
    try {
      await save({
        id: editing?.id ?? '',
        name,
        folder: folder.trim() || null,
        sql: editing ? editing.sql : sql,
        connectionId: editing ? editing.connectionId : connectionId,
      });
      onClose();
    } catch (e) {
      setError(ipc.errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={editing ? 'Rename saved query' : 'Save query'}
      width={430}
    >
      <div className="flex flex-col gap-3">
        <Field label="Name" hint="Left blank, the first line of the SQL is used">
          <input
            className="input"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Daily active users"
            autoFocus
            onKeyDown={(e) => {
              if (e.key === 'Enter') onSave();
            }}
          />
        </Field>

        <Field label="Folder" hint="Optional — type a new name or pick an existing one">
          <input
            className="input"
            value={folder}
            onChange={(e) => setFolder(e.target.value)}
            placeholder="Reports"
            list="faro-folders"
          />
          <datalist id="faro-folders">
            {existingFolders.map((f) => (
              <option key={f} value={f} />
            ))}
          </datalist>
        </Field>

        {!editing && (
          <div>
            <span className="label">SQL</span>
            <pre
              className="selectable max-h-28 overflow-auto rounded-md p-2 font-mono text-[11px]"
              style={{ background: 'var(--bg-inset)', color: 'var(--text-muted)' }}
            >
              {sql.trim() || '(empty)'}
            </pre>
          </div>
        )}

        {error && <ErrorBanner message={error} onDismiss={() => setError(null)} />}

        <div className="mt-1 flex justify-end gap-2">
          <button className="btn btn-ghost" onClick={onClose} type="button">
            Cancel
          </button>
          <button
            className="btn btn-primary"
            onClick={onSave}
            disabled={busy || (!editing && !sql.trim())}
            type="button"
          >
            {editing ? 'Rename' : 'Save'}
          </button>
        </div>
      </div>
    </Modal>
  );
}
