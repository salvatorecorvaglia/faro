import { beforeEach, describe, expect, it } from 'vitest';

import { useConfirmStore } from '@/state/confirm';
import { useTabs } from '@/state/tabs';

const reset = () => useTabs.setState({ tabs: [], activeId: null });

/** Answer the currently pending confirmDialog() request, as ConfirmHost would. */
function answerConfirm(ok: boolean) {
  const request = useConfirmStore.getState().request;
  request?.resolve(ok);
  useConfirmStore.setState({ request: null });
}

describe('tab store', () => {
  beforeEach(reset);

  it('opens a query tab and focuses it', () => {
    const id = useTabs.getState().openQueryTab('c1', 'SELECT 1');
    const { tabs, activeId } = useTabs.getState();
    expect(tabs).toHaveLength(1);
    expect(activeId).toBe(id);
    expect(tabs[0]!.sql).toBe('SELECT 1');
  });

  it('reuses the tab for a table already open', () => {
    // Clicking the same table repeatedly should not stack duplicates.
    const table = { schema: 'public', name: 'users' };
    const a = useTabs.getState().openTableTab('c1', table);
    const b = useTabs.getState().openTableTab('c1', { ...table });
    expect(a).toBe(b);
    expect(useTabs.getState().tabs).toHaveLength(1);
  });

  it('treats same-named tables in different schemas as distinct', () => {
    useTabs.getState().openTableTab('c1', { schema: 'public', name: 'users' });
    useTabs.getState().openTableTab('c1', { schema: 'audit', name: 'users' });
    expect(useTabs.getState().tabs).toHaveLength(2);
  });

  it('treats the same table on different connections as distinct', () => {
    const table = { schema: 'public', name: 'users' };
    useTabs.getState().openTableTab('c1', table);
    useTabs.getState().openTableTab('c2', table);
    expect(useTabs.getState().tabs).toHaveLength(2);
  });

  it('focuses the left neighbour when the active tab closes', async () => {
    const a = useTabs.getState().openQueryTab('c1');
    const b = useTabs.getState().openQueryTab('c1');
    const c = useTabs.getState().openQueryTab('c1');
    await useTabs.getState().setActive(c);

    await useTabs.getState().closeTab(c);
    expect(useTabs.getState().activeId).toBe(b);

    await useTabs.getState().closeTab(b);
    expect(useTabs.getState().activeId).toBe(a);
  });

  it('keeps focus when closing a background tab', async () => {
    const a = useTabs.getState().openQueryTab('c1');
    const b = useTabs.getState().openQueryTab('c1');
    await useTabs.getState().setActive(b);

    await useTabs.getState().closeTab(a);
    expect(useTabs.getState().activeId).toBe(b);
  });

  it('clears the active id when the last tab closes', async () => {
    const a = useTabs.getState().openQueryTab('c1');
    await useTabs.getState().closeTab(a);
    expect(useTabs.getState().activeId).toBeNull();
    expect(useTabs.getState().tabs).toHaveLength(0);
  });

  it('updates only the targeted tab', () => {
    const a = useTabs.getState().openQueryTab('c1');
    const b = useTabs.getState().openQueryTab('c1');
    useTabs.getState().update(a, { sql: 'SELECT 42' });

    const tabs = useTabs.getState().tabs;
    expect(tabs.find((t) => t.id === a)!.sql).toBe('SELECT 42');
    expect(tabs.find((t) => t.id === b)!.sql).toBe('');
  });
});

describe('leaving a tab with staged edits', () => {
  // App mounts only the active tab, so switching away unmounts TableTab and
  // takes its `edits` with it. Refresh/sort/filter/paging all confirmed first;
  // switching and closing did not, and silently destroyed the work.

  beforeEach(() => {
    useTabs.setState({ tabs: [], activeId: null });
    useConfirmStore.setState({ request: null });
  });

  async function twoTabs() {
    const a = useTabs.getState().openQueryTab(null, 'select 1', 'A');
    const b = useTabs.getState().openQueryTab(null, 'select 2', 'B');
    // Neither tab is dirty yet, so this never actually prompts — just needs
    // awaiting so `activeId` has settled before the test's own assertions.
    await useTabs.getState().setActive(a);
    return { a, b };
  }

  it('switches freely when nothing is staged', async () => {
    const { a, b } = await twoTabs();
    expect(useTabs.getState().activeId).toBe(a);
    await useTabs.getState().setActive(b);
    expect(useTabs.getState().activeId).toBe(b);
  });

  it('asks before switching away from a dirty tab', async () => {
    const { a, b } = await twoTabs();
    useTabs.getState().update(a, { dirty: true });

    const switching = useTabs.getState().setActive(b);
    expect(useConfirmStore.getState().request?.message).toContain('"A"');
    answerConfirm(true);
    await switching;

    expect(useTabs.getState().activeId).toBe(b);
  });

  it('stays put when the user declines', async () => {
    const { a } = await twoTabs();
    useTabs.getState().update(a, { dirty: true });

    const switching = useTabs.getState().setActive(useTabs.getState().tabs[1]!.id);
    answerConfirm(false);
    await switching;

    expect(useTabs.getState().activeId).toBe(a);
  });

  it('asks before closing a dirty tab and keeps it when declined', async () => {
    const { a } = await twoTabs();
    useTabs.getState().update(a, { dirty: true });

    const closing = useTabs.getState().closeTab(a);
    answerConfirm(false);
    await closing;

    expect(useTabs.getState().tabs.map((t) => t.id)).toContain(a);
  });

  it('does not prompt when re-selecting the tab already active', async () => {
    const { a } = await twoTabs();
    useTabs.getState().update(a, { dirty: true });
    await useTabs.getState().setActive(a);

    expect(useConfirmStore.getState().request).toBeNull();
  });
});
