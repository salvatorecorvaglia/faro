import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { useEffect, useState } from 'react';

import { ErrorBanner, Field, Modal, Spinner } from '@/components/ui';
import * as ipc from '@/ipc';
import type { ConnectionConfig, Engine, EngineInfo, SslMode } from '@/ipc/types';
import { useConnections } from '@/state/connections';

const blank = (): ConnectionConfig => ({
  id: '',
  name: '',
  engine: 'postgres',
  host: 'localhost',
  port: 5432,
  username: '',
  database: '',
  filePath: null,
  sslMode: 'prefer',
  color: null,
  readOnly: false,
});

/**
 * What "Open read-only" actually guarantees for a given engine.
 *
 * Faro refuses edits, imports and restores on a read-only connection whatever
 * the engine, and blocks non-read statements in the editor by inspecting them.
 * Most engines can also be told to enforce it themselves, which additionally
 * catches a write Faro's inspection would miss — a function call that writes,
 * say. SQL Server has no equivalent session setting, so there the promise rests
 * on Faro alone and the wording should not imply otherwise.
 */
function readOnlyHint(engine: Engine): string {
  const faroSide = 'Faro will refuse edits, imports, restores and write statements.';
  return engine === 'sqlserver'
    ? `${faroSide} SQL Server has no server-side read-only session, so this is enforced by Faro only.`
    : `${faroSide} The database is asked to enforce it as well.`;
}

export function ConnectionDialog({
  open,
  onClose,
  editing,
}: {
  open: boolean;
  onClose: () => void;
  editing: ConnectionConfig | null;
}) {
  const save = useConnections((s) => s.save);
  const connect = useConnections((s) => s.connect);

  const [config, setConfig] = useState<ConnectionConfig>(blank);
  /**
   * `undefined` means "unchanged" and is what gets sent when editing an
   * existing connection whose password the user did not retype — sending an
   * empty string would wipe the stored password.
   */
  const [password, setPassword] = useState<string | undefined>(undefined);
  const [engines, setEngines] = useState<EngineInfo[]>([]);
  const [busy, setBusy] = useState<'test' | 'save' | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tested, setTested] = useState(false);

  useEffect(() => {
    ipc
      .listEngines()
      .then(setEngines)
      .catch(() => setEngines([]));
  }, []);

  useEffect(() => {
    if (!open) return;
    setConfig(editing ? { ...editing } : blank());
    setPassword(editing ? undefined : '');
    setError(null);
    setTested(false);
  }, [open, editing]);

  const engine = engines.find((e) => e.id === config.engine);
  const fileBased = engine?.fileBased ?? false;
  const supported = engine?.supported ?? true;

  const patch = (p: Partial<ConnectionConfig>) => {
    setConfig((c) => ({ ...c, ...p }));
    setTested(false);
  };

  function onEngineChange(id: Engine) {
    const info = engines.find((e) => e.id === id);
    // Only overwrite the port if it still holds the previous engine's default,
    // so a deliberately-set port survives an engine change.
    const wasDefault = engines.some((e) => e.defaultPort === config.port);
    patch({
      engine: id,
      port: wasDefault && info ? info.defaultPort : config.port,
    });
  }

  async function pickFile() {
    // Offer the extensions that match the chosen engine — a DuckDB user
    // shouldn't have to fall back to "All files" to find their own database.
    const named =
      config.engine === 'duckdb'
        ? { name: 'DuckDB database', extensions: ['duckdb', 'ddb', 'db'] }
        : { name: 'SQLite database', extensions: ['db', 'sqlite', 'sqlite3', 'db3'] };

    const picked = await openFileDialog({
      multiple: false,
      filters: [named, { name: 'All files', extensions: ['*'] }],
    });
    if (typeof picked === 'string') patch({ filePath: picked });
  }

  async function onTest() {
    setBusy('test');
    setError(null);
    try {
      await ipc.testConnection(config, password);
      setTested(true);
    } catch (e) {
      setError(ipc.errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  async function onSave(thenConnect: boolean) {
    setBusy('save');
    setError(null);
    try {
      const saved = await save(config, password);
      if (thenConnect) await connect(saved.id);
      onClose();
    } catch (e) {
      setError(ipc.errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  const canSubmit = fileBased ? !!config.filePath : config.host.trim().length > 0;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={editing ? 'Edit connection' : 'New connection'}
      width={480}
    >
      <div className="flex flex-col gap-3">
        <Field label="Database type">
          <select
            className="input"
            value={config.engine}
            onChange={(e) => onEngineChange(e.target.value as Engine)}
          >
            {engines.map((e) => (
              <option key={e.id} value={e.id}>
                {e.name}
                {e.supported ? '' : ' — coming soon'}
              </option>
            ))}
          </select>
        </Field>

        {!supported && (
          <p
            className="rounded-md px-2.5 py-2 text-[12px]"
            style={{
              background: 'color-mix(in srgb, var(--warning) 14%, transparent)',
              color: 'var(--warning)',
            }}
          >
            {engine?.name} support is not implemented yet. You can save this connection, but it will
            not open until that driver lands.
          </p>
        )}

        {fileBased ? (
          <Field label="Database file">
            <div className="flex gap-2">
              <input
                className="input font-mono"
                value={config.filePath ?? ''}
                placeholder={
                  config.engine === 'duckdb' ? '/path/to/database.duckdb' : '/path/to/database.db'
                }
                onChange={(e) => patch({ filePath: e.target.value })}
              />
              <button className="btn btn-outline shrink-0" onClick={pickFile} type="button">
                Browse…
              </button>
            </div>
          </Field>
        ) : (
          <>
            <div className="flex gap-2">
              <div className="flex-1">
                <Field label="Host">
                  <input
                    className="input"
                    value={config.host}
                    onChange={(e) => patch({ host: e.target.value })}
                    placeholder="localhost"
                  />
                </Field>
              </div>
              <div className="w-24">
                <Field label="Port">
                  <input
                    className="input"
                    type="number"
                    value={config.port || ''}
                    onChange={(e) => patch({ port: Number(e.target.value) || 0 })}
                  />
                </Field>
              </div>
            </div>

            <div className="flex gap-2">
              <div className="flex-1">
                <Field label="User">
                  <input
                    className="input"
                    value={config.username}
                    onChange={(e) => patch({ username: e.target.value })}
                    autoComplete="off"
                  />
                </Field>
              </div>
              <div className="flex-1">
                <Field
                  label="Password"
                  hint={
                    editing && password === undefined
                      ? 'Leave blank to keep the saved password'
                      : undefined
                  }
                >
                  <input
                    className="input"
                    type="password"
                    value={password ?? ''}
                    onChange={(e) => {
                      setPassword(e.target.value);
                      setTested(false);
                    }}
                    autoComplete="new-password"
                  />
                </Field>
              </div>
            </div>

            <div className="flex gap-2">
              <div className="flex-1">
                <Field label="Database">
                  <input
                    className="input"
                    value={config.database}
                    onChange={(e) => patch({ database: e.target.value })}
                  />
                </Field>
              </div>
              <div className="w-40">
                <Field
                  label="SSL"
                  hint={
                    config.sslMode === 'require'
                      ? 'Encrypts, but accepts any certificate'
                      : undefined
                  }
                >
                  <select
                    className="input"
                    value={config.sslMode}
                    onChange={(e) => patch({ sslMode: e.target.value as SslMode })}
                  >
                    <option value="prefer">Prefer</option>
                    <option value="require">Require (no verification)</option>
                    <option value="verifyCa">Verify CA</option>
                    <option value="verifyFull">Verify full</option>
                    <option value="disable">Disable</option>
                  </select>
                </Field>
              </div>
            </div>
          </>
        )}

        <Field label="Display name" hint="Optional — one is generated if left blank">
          <input
            className="input"
            value={config.name}
            onChange={(e) => setConfig((c) => ({ ...c, name: e.target.value }))}
            placeholder={fileBased ? 'My database' : `${config.host}/${config.database}`}
          />
        </Field>

        <div>
          <label
            className="flex items-center gap-2 text-[12px]"
            style={{ color: 'var(--text-muted)' }}
          >
            <input
              type="checkbox"
              checked={config.readOnly}
              onChange={(e) => patch({ readOnly: e.target.checked })}
            />
            Open read-only
          </label>
          {/* Says what is actually guaranteed. Faro blocks every write it can
              identify, and asks the engine to block them too where it can — but
              those are two different strengths of promise and the difference
              matters for a production connection. */}
          <p className="mt-1 text-[11px]" style={{ color: 'var(--text-faint)' }}>
            {readOnlyHint(config.engine)}
          </p>
        </div>

        {error && <ErrorBanner message={error} onDismiss={() => setError(null)} />}
        {tested && !error && (
          <p className="text-[12px]" style={{ color: 'var(--success)' }}>
            Connected successfully.
          </p>
        )}

        <div className="mt-1 flex items-center justify-between gap-2">
          <button
            className="btn btn-outline"
            onClick={onTest}
            disabled={!!busy || !canSubmit || !supported}
            type="button"
          >
            {busy === 'test' ? <Spinner /> : null}
            Test
          </button>
          <div className="flex gap-2">
            <button className="btn btn-ghost" onClick={onClose} type="button">
              Cancel
            </button>
            <button
              className="btn btn-outline"
              onClick={() => onSave(false)}
              disabled={!!busy || !canSubmit}
              type="button"
            >
              Save
            </button>
            <button
              className="btn btn-primary"
              onClick={() => onSave(true)}
              disabled={!!busy || !canSubmit || !supported}
              type="button"
            >
              {busy === 'save' ? <Spinner /> : null}
              Save &amp; connect
            </button>
          </div>
        </div>
      </div>
    </Modal>
  );
}
