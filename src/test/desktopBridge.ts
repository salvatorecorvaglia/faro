import { invoke } from '@tauri-apps/api/core';
import * as ipc from '@/ipc';
import { useConnections } from '@/state/connections';
import { confirmExit } from '@/state/exit';
import { useTabs } from '@/state/tabs';
import { useUpdaterStore } from '@/state/updater';

export async function start() {
  const metrics = await invoke('smoke_action', { action: 'startup' });
  const paths = await invoke<{ database: string; csv: string; sql: string; backup: string }>(
    'smoke_action',
    { action: 'fixtures' },
  );
  const config: ipc.ConnectionConfig = {
    id: 'smoke-sqlite',
    name: 'Smoke database',
    engine: 'sqlite',
    host: '',
    port: 0,
    username: '',
    database: '',
    filePath: paths.database,
    sslMode: 'disable',
    color: null,
    readOnly: false,
  };
  // Private disposable database; no saved connections or user data are opened.
  await ipc.saveConnection(config);
  await ipc.connect(config.id);
  await ipc.runQuery(
    config.id,
    "CREATE TABLE smoke_rows (id INTEGER PRIMARY KEY, name TEXT); INSERT INTO smoke_rows VALUES (1, 'Original');",
    'smoke-seed',
    100,
  );
  const backup = await ipc.backupDatabase(
    config.id,
    paths.backup,
    { tables: [], includeSchema: true, includeData: true, dropExisting: false },
    'smoke-backup',
  );
  if (!backup.consistentSnapshot) throw new Error('Native backup did not use a snapshot');
  async function beginOperation(kind: 'import' | 'restore') {
    await ipc.runQuery(config.id, 'BEGIN IMMEDIATE', `lock-${kind}`, 100);
    const queryId = `smoke-${kind}`;
    let settled = false;
    const operation = (
      kind === 'import'
        ? ipc.importFile(
            config.id,
            { schema: null, name: 'smoke_rows' },
            paths.csv,
            {
              format: 'csv',
              hasHeader: true,
              mappings: [
                { sourceIndex: 0, targetColumn: 'id' },
                { sourceIndex: 1, targetColumn: 'name' },
              ],
              nullTokens: [''],
            },
            queryId,
          )
        : ipc.restoreDatabase(config.id, paths.sql, { stopOnError: true }, queryId)
    ).then(
      () => {
        settled = true;
        return null;
      },
      (error) => {
        settled = true;
        return ipc.toFaroError(error);
      },
    );
    const deadline = performance.now() + 3000;
    while (!(await ipc.runningOperations())) {
      if (settled || performance.now() > deadline) throw new Error(`${kind} did not stay active`);
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    return {
      stop: async () => {
        await ipc.cancelQuery(config.id, queryId);
        await ipc.runQuery(config.id, 'ROLLBACK', `unlock-${kind}`, 100);
        const error = await operation;
        if (
          error?.kind !== 'cancelled' &&
          !(kind === 'restore' && error?.message.startsWith('Restore cancelled.'))
        )
          throw new Error(`${kind} did not report cancellation`);
        const check = await ipc.runQuery(
          config.id,
          'SELECT COUNT(*) FROM smoke_rows',
          `check-${kind}`,
          10,
        );
        const outcome = check.statements[0]?.outcome;
        if (
          outcome?.type !== 'rows' ||
          outcome.rows[0]?.[0]?.kind !== 'int' ||
          outcome.rows[0][0].value !== 1
        )
          throw new Error(`${kind} changed rows despite cancellation`);
      },
    };
  }
  async function openLargeResult() {
    const sql =
      "WITH RECURSIVE series(n) AS (SELECT 0 UNION ALL SELECT n + 1 FROM series WHERE n < 49999) SELECT n AS id, printf('Item %05d', 49999-n) AS name FROM series";
    const result = await ipc.runQuery(config.id, sql, 'smoke-large', 50000);
    const query = useTabs.getState().openQueryTab(config.id, sql, 'Large result');
    useTabs.getState().update(query, { results: result.statements, resultQueryId: 'smoke-large' });
  }
  useConnections.setState({ items: [{ ...config, connected: true, hasPassword: false }] });
  const id = useTabs.getState().openTableTab(config.id, { schema: null, name: 'smoke_rows' });
  function showReadyUpdate() {
    useUpdaterStore.setState({ status: 'ready', version: 'smoke', dismissed: false, error: null });
  }
  Object.assign(window, {
    __FARO_SMOKE__: {
      id,
      metrics,
      tabs: useTabs,
      confirmExit,
      beginOperation,
      openLargeResult,
      showReadyUpdate,
    },
  });
}
