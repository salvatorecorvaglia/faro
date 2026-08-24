import { save } from '@tauri-apps/plugin-dialog';
import { useState } from 'react';

import { ErrorBanner, Field, Modal, Spinner } from '@/components/ui';
import { useAsyncAction } from '@/hooks/useAsyncAction';
import * as ipc from '@/ipc';
import type { ExportFormat, ResultSet, TableRef } from '@/ipc/types';

let exportCounter = 0;

const FORMATS: { id: ExportFormat; label: string; hint: string }[] = [
  { id: 'csv', label: 'CSV', hint: 'Comma-separated, opens anywhere' },
  { id: 'tsv', label: 'TSV', hint: 'Tab-separated, pastes into spreadsheets' },
  { id: 'json', label: 'JSON', hint: 'Array of objects' },
  { id: 'sql', label: 'SQL', hint: 'INSERT statements' },
  { id: 'xlsx', label: 'Excel', hint: 'Native .xlsx workbook' },
];

/**
 * Export the current result, or the whole table it came from.
 *
 * The distinction matters: the grid holds one page, so exporting "the result"
 * of a large table would quietly stop at 1000 rows. Exporting the table
 * re-reads it from the database in batches.
 */
export function ExportDialog({
  open,
  onClose,
  result,
  connectionId,
  table,
  defaultName,
}: {
  open: boolean;
  onClose: () => void;
  result: ResultSet | null;
  connectionId: string | null;
  /** Set when the export can target a whole table rather than a result set. */
  table?: TableRef | null;
  defaultName: string;
}) {
  const [format, setFormat] = useState<ExportFormat>('csv');
  const [includeHeader, setIncludeHeader] = useState(true);
  const [sanitizeFormulas, setSanitizeFormulas] = useState(true);
  const [wholeTable, setWholeTable] = useState(!!table);
  const { busy, error, done, reset, run } = useAsyncAction();

  const canExportTable = !!table && !!connectionId;
  const truncated = result?.truncated ?? false;

  async function onExport() {
    // The whole flow — including choosing a file — runs inside `run` so a
    // failure at any point (suggesting a name, the picker itself) is caught
    // and shown the same way a failure in the actual export would be.
    await run(async () => {
      const suggested = await ipc.suggestedExportName(defaultName, format);
      const path = await save({
        defaultPath: suggested,
        filters: [{ name: format.toUpperCase(), extensions: [format] }],
      });
      if (!path) return;

      const options = {
        format,
        includeHeader,
        tableName: table?.name ?? null,
        sanitizeFormulas,
      };
      const outcome =
        wholeTable && canExportTable
          ? await ipc.exportTable(connectionId!, table!, path, options, `exp-${++exportCounter}`)
          : await ipc.exportResult(path, result!, options);

      return `Wrote ${outcome.rows.toLocaleString()} rows to ${outcome.path}`;
    });
  }

  return (
    <Modal open={open} onClose={onClose} title="Export" width={440}>
      <div className="flex flex-col gap-3">
        <Field label="Format">
          <div className="flex flex-wrap gap-1">
            {FORMATS.map((f) => (
              <button
                key={f.id}
                className="btn btn-outline"
                onClick={() => setFormat(f.id)}
                style={
                  format === f.id
                    ? { borderColor: 'var(--accent)', color: 'var(--accent)' }
                    : undefined
                }
                title={f.hint}
                type="button"
              >
                {f.label}
              </button>
            ))}
          </div>
        </Field>

        {canExportTable && (
          <div className="flex flex-col gap-1.5">
            <label className="flex items-center gap-2 text-[12px]">
              <input type="radio" checked={wholeTable} onChange={() => setWholeTable(true)} />
              Entire table
            </label>
            <label className="flex items-center gap-2 text-[12px]">
              <input type="radio" checked={!wholeTable} onChange={() => setWholeTable(false)} />
              Just this page
              <span style={{ color: 'var(--text-faint)' }}>
                ({(result?.rows.length ?? 0).toLocaleString()} rows)
              </span>
            </label>
            {!wholeTable && truncated && (
              // Without this, someone exports 1000 rows of a million-row table
              // and has no way to know the file is incomplete.
              <p className="text-[11px]" style={{ color: 'var(--warning)' }}>
                This page is truncated — the table has more rows than are shown.
              </p>
            )}
          </div>
        )}

        {format !== 'json' && format !== 'sql' && (
          <label className="flex items-center gap-2 text-[12px]">
            <input
              type="checkbox"
              checked={includeHeader}
              onChange={(e) => setIncludeHeader(e.target.checked)}
            />
            Include a header row
          </label>
        )}

        {(format === 'csv' || format === 'tsv') && (
          <label className="flex items-start gap-2 text-[12px]">
            <input
              type="checkbox"
              checked={sanitizeFormulas}
              onChange={(e) => setSanitizeFormulas(e.target.checked)}
            />
            <span>
              Escape spreadsheet formulas
              <span className="block text-[11px]" style={{ color: 'var(--text-muted)' }}>
                Prefixes cells starting with = + - @ so Excel shows them as text instead of running
                them. Turn off only when feeding the file to another program.
              </span>
            </span>
          </label>
        )}

        {error && <ErrorBanner message={error} onDismiss={reset} />}
        {done && (
          <p className="break-all text-[11.5px]" style={{ color: 'var(--success)' }}>
            {done}
          </p>
        )}

        <div className="mt-1 flex justify-end gap-2">
          <button className="btn btn-ghost" onClick={onClose} type="button">
            Close
          </button>
          <button
            className="btn btn-primary"
            onClick={onExport}
            disabled={busy || (!result && !wholeTable)}
            type="button"
          >
            {busy && <Spinner size={11} />}
            Choose file…
          </button>
        </div>
      </div>
    </Modal>
  );
}
