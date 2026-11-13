import { expect, test } from '@playwright/test';

test('production shell and deferred editor load without errors', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.addInitScript(() => {
    let counter = 0;
    const config = {
      id: 'browser',
      name: 'Test connection',
      engine: 'sqlite',
      filePath: ':memory:',
      host: '',
      port: 0,
      database: '',
      username: '',
      sslMode: 'disable',
      color: null,
      readOnly: false,
      connected: true,
      hasPassword: false,
    };
    Object.assign(window, {
      __TAURI_INTERNALS__: {
        transformCallback: () => ++counter,
        unregisterCallback: () => {},
        invoke: async (command: string) => {
          if (command === 'plugin:event|listen') return ++counter;
          if (command === 'list_connection_status') return [config];
          if (command === 'keychain_available') return true;
          if (
            command === 'list_schemas' ||
            command === 'list_engines' ||
            command === 'schema_snapshot' ||
            command === 'list_saved_queries' ||
            command === 'list_history'
          )
            return [];
          if (command === 'statement_at_cursor') return null;
          return undefined;
        },
      },
    });
  });
  await page.goto('/');
  await expect(page.getByText('No tab open', { exact: true })).toBeVisible();
  const shell = await page.evaluate(() => ({
    readyMs: performance.now(),
    scripts: performance
      .getEntriesByType('resource')
      .filter((entry) => entry.name.endsWith('.js'))
      .map((entry) => entry.name.split('/').at(-1)),
  }));
  expect(
    shell.scripts.some(
      (script) => script!.startsWith('QueryTab-') || script!.startsWith('format-'),
    ),
  ).toBe(false);
  await page.getByRole('button', { name: 'New query tab' }).click();
  await expect(page.locator('.cm-editor')).toBeVisible();
  const editor = await page.evaluate(() => ({
    readyMs: performance.now(),
    scripts: performance
      .getEntriesByType('resource')
      .filter((entry) => entry.name.endsWith('.js'))
      .map((entry) => entry.name.split('/').at(-1)),
  }));
  expect(editor.scripts.some((script) => script!.startsWith('QueryTab-'))).toBe(true);
  expect(editor.scripts.some((script) => script!.startsWith('format-'))).toBe(false);
  const close = page.getByRole('button', { name: /^Close Query/ }).first();
  await close.focus();
  await expect(close).toHaveCSS('opacity', '0.6');
  expect(errors).toEqual([]);
  console.log('production startup browser measurement', {
    shellReadyMs: shell.readyMs,
    editorReadyMs: editor.readyMs,
  });
});
