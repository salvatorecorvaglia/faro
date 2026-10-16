import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import App from '@/App';
import { useConnections } from '@/state/connections';
import { useTabs } from '@/state/tabs';
import { mockInvoke } from '@/test/ipc';

/**
 * How often the shell re-renders, which is a performance property rather than
 * a behavioural one — so the heavy children are replaced with counters.
 *
 * Every editor keystroke writes the tab's SQL into the store. `App` used to
 * subscribe to the whole `tabs` array, so each key re-rendered the sidebar and
 * its schema tree, the palette and every dialog host, for a change only the
 * editor needed.
 */
const renders = vi.hoisted(() => ({ sidebar: 0 }));

vi.mock('@/features/connections/Sidebar', () => ({
  Sidebar: () => {
    renders.sidebar += 1;
    return <aside aria-label="sidebar" />;
  },
}));

vi.mock('@/features/editor/Editor', () => ({
  Editor: ({ value }: { value: string }) => (
    <textarea aria-label="sql editor" value={value} readOnly />
  ),
}));

beforeEach(() => {
  renders.sidebar = 0;
  useTabs.setState({ tabs: [], activeId: null });
  useConnections.setState({ items: [], loading: false, connecting: new Set(), error: null });
  mockInvoke({
    list_history: () => [],
    list_saved_queries: () => [],
    schema_snapshot: () => [],
  });
});

describe('App shell re-rendering', () => {
  it('does not re-render the sidebar while the user types in a query tab', () => {
    render(<App />);
    let id = '';
    act(() => {
      id = useTabs.getState().openQueryTab(null, 'SELECT', 'Query 1');
    });
    const before = renders.sidebar;

    // What the editor does on each keystroke.
    act(() => {
      for (const sql of ['SELECT 1', 'SELECT 12', 'SELECT 123']) {
        useTabs.getState().update(id, { sql });
      }
    });

    expect(renders.sidebar).toBe(before);
  });
});
