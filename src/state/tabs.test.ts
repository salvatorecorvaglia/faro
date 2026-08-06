import { beforeEach, describe, expect, it } from 'vitest';

import { useTabs } from './tabs';

const reset = () => useTabs.setState({ tabs: [], activeId: null });

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

  it('focuses the left neighbour when the active tab closes', () => {
    const a = useTabs.getState().openQueryTab('c1');
    const b = useTabs.getState().openQueryTab('c1');
    const c = useTabs.getState().openQueryTab('c1');
    useTabs.getState().setActive(c);

    useTabs.getState().closeTab(c);
    expect(useTabs.getState().activeId).toBe(b);

    useTabs.getState().closeTab(b);
    expect(useTabs.getState().activeId).toBe(a);
  });

  it('keeps focus when closing a background tab', () => {
    const a = useTabs.getState().openQueryTab('c1');
    const b = useTabs.getState().openQueryTab('c1');
    useTabs.getState().setActive(b);

    useTabs.getState().closeTab(a);
    expect(useTabs.getState().activeId).toBe(b);
  });

  it('clears the active id when the last tab closes', () => {
    const a = useTabs.getState().openQueryTab('c1');
    useTabs.getState().closeTab(a);
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
