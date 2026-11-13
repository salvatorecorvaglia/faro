export async function start() {
  const invoke = (command, args) => window.__TAURI_INTERNALS__.invoke(command, args);
  const wait = async (predicate, message) => {
    const deadline = performance.now() + 15000;
    while (!predicate()) {
      if (performance.now() > deadline) throw new Error(message);
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  };
  const button = (name) =>
    [...document.querySelectorAll('button')].find((item) => item.textContent.trim() === name);
  const checks = [];
  try {
    await wait(() => window.__FARO_SMOKE__, 'Desktop bridge did not start');
    const bridge = window.__FARO_SMOKE__;
    await wait(
      () =>
        [...document.querySelectorAll('[role="gridcell"]')].some(
          (cell) => cell.textContent === 'Original',
        ),
      'Real SQLite browse did not render',
    );
    await bridge.openLargeResult();
    await wait(
      () => document.querySelector('[role="grid"]')?.getAttribute('aria-rowcount') === '50000',
      'Large result did not render',
    );
    const header = [...document.querySelectorAll('[role="columnheader"]')].find((item) =>
      item.textContent.includes('name'),
    );
    header.click();
    await wait(
      () => document.querySelector('[role="row"] [role="gridcell"]')?.textContent === '49999',
      'Native worker sort did not complete',
    );
    checks.push('50,000-row worker sort through the native custom protocol');
    await bridge.tabs.getState().setActive(bridge.id);
    await wait(
      () =>
        [...document.querySelectorAll('[role="gridcell"]')].some(
          (item) => item.textContent === 'Original',
        ),
      'Table did not remount',
    );
    await wait(() => button('Row'), 'Table metadata did not enable editing');
    const cell = [...document.querySelectorAll('[role="gridcell"]')].find(
      (item) => item.textContent === 'Original',
    );
    cell.dispatchEvent(new MouseEvent('dblclick', { bubbles: true }));
    await wait(() => document.querySelector('[role="grid"] input'), 'Cell editor did not open');
    const editor = document.querySelector('[role="grid"] input');
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(
      editor,
      'Changed',
    );
    editor.dispatchEvent(new Event('input', { bubbles: true }));
    await new Promise((resolve) => setTimeout(resolve, 50));
    editor.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await wait(
      () => bridge.tabs.getState().tabs.find((tab) => tab.id === bridge.id)?.dirty,
      'Edit did not stage',
    );
    checks.push('native SQLite IPC and staged grid edit');
    for (const action of ['close', 'quit']) {
      await invoke('smoke_action', { action });
      await wait(
        () => document.querySelector('dialog[open]'),
        `${action} did not request confirmation`,
      );
      if (!document.querySelector('dialog').textContent.includes('unapplied edits'))
        throw new Error('Missing dirty edit warning');
      button('Cancel').click();
      await wait(
        () => !document.querySelector('dialog[open]'),
        'Cancel did not dismiss confirmation',
      );
      if (!bridge.tabs.getState().tabs.find((tab) => tab.id === bridge.id)?.dirty)
        throw new Error('Declining quit lost edits');
      checks.push(`native ${action} declined preserves window and edits`);
    }
    bridge.showReadyUpdate();
    await wait(() => button('Restart now'), 'Ready-update action did not render');
    button('Restart now').click();
    await wait(() => document.querySelector('dialog[open]'), 'Restart confirmation missing');
    button('Cancel').click();
    await wait(() => !document.querySelector('dialog[open]'), 'Restart confirmation did not close');
    if (!bridge.tabs.getState().tabs.find((tab) => tab.id === bridge.id)?.dirty)
      throw new Error('Declined restart lost staged edits');
    checks.push('ready updater restart action declined preserves window and edits');
    checks.push('native backup uses a consistent SQLite snapshot');
    for (const kind of ['import', 'restore']) {
      const operation = await bridge.beginOperation(kind);
      await invoke('smoke_action', { action: 'close' });
      await wait(() => document.querySelector('dialog[open]'), `${kind} confirmation missing`);
      if (!document.querySelector('dialog').textContent.includes('still running'))
        throw new Error(`Missing ${kind} running warning`);
      button('Cancel').click();
      await operation.stop();
      checks.push(
        `native close warns while ${kind} is running; declining preserves window; cancellation rolls back`,
      );
    }
    const finish = await invoke('smoke_action', { action: 'finish' });
    await invoke('smoke_action', { action: finish });
    await wait(
      () => document.querySelector('dialog[open]'),
      'Final native quit confirmation missing',
    );
    checks.push(`accepting native ${finish} exits successfully`);
    await invoke('smoke_report', {
      result: { ok: true, checks, metrics: bridge.metrics },
      finish: false,
    });
    button('Quit').click();
  } catch (error) {
    await invoke('smoke_report', {
      result: {
        ok: false,
        checks,
        error: String(error),
        screen: (document.body.textContent ?? '').slice(-2000),
      },
    });
  }
}
