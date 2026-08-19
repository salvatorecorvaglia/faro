import { open as openFile } from '@tauri-apps/plugin-dialog';
import { useState } from 'react';

import { ErrorBanner, Field, Modal, Spinner } from '@/components/ui';
import { useAsyncAction } from '@/hooks/useAsyncAction';
import * as ipc from '@/ipc';
import type {
  ColumnDetail,
  ColumnMapping,
  ImportFormat,
  ImportPreview,
  TableRef,
} from '@/ipc/types';

/**
 * Import a file into a table.
 *
 * Deliberately a three-step flow — choose a file, check the mapping, then
 * import — because a wrong mapping writes plausible-looking data into the
 * wrong columns, which is both easy to miss and hard to undo. The preview
 * exists so that mistake is caught before anything is written.
 */
export function ImportDialog({
  open,
  onClose,
  connectionId,
  table,
  columns,
  onImported,
}: {
  open: boolean;
  onClose: () => void;
  connectionId: string | null;
  table: TableRef | null;
  columns: ColumnDetail[];
  onImported: () => void;
}) {
  const [path, setPath] = useState<string | null>(null);
  const [hasHeader, setHasHeader] = useState(true);
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [mappings, setMappings] = useState<Record<number, string>>({});
  const { busy, error, done, reset: resetStatus, run } = useAsyncAction();

  function reset() {
    setPath(null);
    setPreview(null);
    setMappings({});
    resetStatus();
  }

  async function choose() {
    const picked = await openFile({
      multiple: false,
      filters: [
        { name: 'Data files', extensions: ['csv', 'tsv', 'json', 'xlsx'] },
        { name: 'All files', extensions: ['*'] },
      ],
    });
    if (typeof picked !== 'string') return;
    setPath(picked);
    await load(picked, hasHeader);
  }

  async function load(file: string, header: boolean) {
    setPreview(null);
    await run(async () => {
      const p = await ipc.previewImport(file, header);
      setPreview(p);
      // Pre-map columns whose names match, case-insensitively. Guessing beyond
      // an exact match would be more likely to mislead than to help.
      const guessed: Record<number, string> = {};
      p.columns.forEach((source, i) => {
        const hit = columns.find((c) => c.name.toLowerCase() === source.trim().toLowerCase());
        if (hit) guessed[i] = hit.name;
      });
      setMappings(guessed);
    });
  }

  async function runImport() {
    if (!path || !connectionId || !table) return;
    const list: ColumnMapping[] = Object.entries(mappings)
      .filter(([, target]) => !!target)
      .map(([index, target]) => ({ sourceIndex: Number(index), targetColumn: target }));

    // Naming the target explicitly: a wrong mapping writes plausible-looking
    // data into the wrong columns, which is both easy to miss and hard to
    // undo — the same reason restore and apply confirm before they write.
    const rows = preview?.totalRows?.toLocaleString() ?? 'these';
    if (!confirm(`Import ${rows} rows into "${table.name}"?\n\nThis modifies that table.`)) {
      return;
    }

    const ok = await run(async () => {
      const outcome = await ipc.importFile(connectionId, table, path, {
        format: formatOf(path),
        hasHeader,
        mappings: list,
        nullTokens: [''],
      });
      return `Imported ${outcome.rows.toLocaleString()} rows.`;
    });
    if (ok) onImported();
  }

  const mappedCount = Object.values(mappings).filter(Boolean).length;

  return (
    <Modal
      open={open}
      onClose={() => {
        reset();
        onClose();
      }}
      title={`Import into ${table?.name ?? 'table'}`}
      width={620}
    >
      <div className="flex flex-col gap-3">
        <div className="flex items-end gap-2">
          <div className="min-w-0 flex-1">
            <Field label="File">
              <input
                className="input truncate font-mono"
                value={path ?? ''}
                placeholder="Choose a .csv, .tsv, .json or .xlsx file"
                readOnly
              />
            </Field>
          </div>
          <button className="btn btn-outline" onClick={choose} disabled={busy} type="button">
            Browse…
          </button>
        </div>

        <label className="flex items-center gap-2 text-[12px]">
          <input
            type="checkbox"
            checked={hasHeader}
            onChange={(e) => {
              setHasHeader(e.target.checked);
              if (path) load(path, e.target.checked);
            }}
          />
          First row contains column names
        </label>

        {busy && !preview && (
          <div
            className="flex items-center gap-2 text-[12px]"
            style={{ color: 'var(--text-muted)' }}
          >
            <Spinner size={12} /> Reading file…
          </div>
        )}

        {preview && (
          <>
            <div>
              <span className="label">
                Column mapping — {mappedCount} of {preview.columns.length} mapped
              </span>
              <div
                className="max-h-52 overflow-auto rounded-md border"
                style={{ borderColor: 'var(--border)' }}
              >
                <table className="w-full text-[11.5px]">
                  <thead className="sticky top-0" style={{ background: 'var(--bg-inset)' }}>
                    <tr>
                      <th className="px-2 py-1 text-left font-semibold">File column</th>
                      <th className="px-2 py-1 text-left font-semibold">Looks like</th>
                      <th className="px-2 py-1 text-left font-semibold">Sample</th>
                      <th className="px-2 py-1 text-left font-semibold">Import into</th>
                    </tr>
                  </thead>
                  <tbody>
                    {preview.columns.map((source, i) => (
                      <tr key={i} className="border-t" style={{ borderColor: 'var(--border)' }}>
                        <td className="max-w-32 truncate px-2 py-1 font-mono">{source}</td>
                        <td className="px-2 py-1" style={{ color: 'var(--text-faint)' }}>
                          {preview.inferredTypes[i]}
                        </td>
                        <td
                          className="max-w-32 truncate px-2 py-1 font-mono"
                          style={{ color: 'var(--text-muted)' }}
                        >
                          {preview.sampleRows[0]?.[i] ?? ''}
                        </td>
                        <td className="px-2 py-1">
                          <select
                            className="input py-0.5 text-[11px]"
                            value={mappings[i] ?? ''}
                            onChange={(e) => setMappings((m) => ({ ...m, [i]: e.target.value }))}
                          >
                            <option value="">— skip —</option>
                            {columns.map((c) => (
                              <option key={c.name} value={c.name}>
                                {c.name}
                              </option>
                            ))}
                          </select>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>

            <p className="text-[11.5px]" style={{ color: 'var(--text-muted)' }}>
              {preview.totalRows?.toLocaleString() ?? '?'} rows will be inserted. Empty fields
              become NULL. This runs as one transaction — if any row fails, nothing is imported.
            </p>
          </>
        )}

        {error && <ErrorBanner message={error} onDismiss={resetStatus} />}
        {done && (
          <p className="text-[12px]" style={{ color: 'var(--success)' }}>
            {done}
          </p>
        )}

        <div className="mt-1 flex justify-end gap-2">
          <button
            className="btn btn-ghost"
            onClick={() => {
              reset();
              onClose();
            }}
            type="button"
          >
            Close
          </button>
          <button
            className="btn btn-primary"
            onClick={runImport}
            disabled={busy || !preview || mappedCount === 0}
            type="button"
          >
            {busy && <Spinner size={11} />}
            Import
          </button>
        </div>
      </div>
    </Modal>
  );
}

function formatOf(path: string): ImportFormat {
  const ext = path.split('.').pop()?.toLowerCase();
  if (ext === 'tsv' || ext === 'tab') return 'tsv';
  if (ext === 'json') return 'json';
  if (ext === 'xlsx' || ext === 'xlsm') return 'xlsx';
  return 'csv';
}
