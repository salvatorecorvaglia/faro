import { confirmDialog } from './confirm';
import { flushTabStorage, useTabs } from './tabs';

/** Shared by native quit and updater restart. */
export async function confirmExit(running: boolean): Promise<boolean> {
  const dirty = useTabs.getState().tabs.filter((tab) => tab.dirty);
  if (dirty.length || running) {
    const costs = [
      dirty.length
        ? `Discard unapplied edits in ${dirty.length} tab${dirty.length === 1 ? '' : 's'}?`
        : '',
      running
        ? 'Database operations are still running. Quitting stops waiting and requests cancellation; the database may still finish work already sent.'
        : '',
    ]
      .filter(Boolean)
      .join('\n\n');
    if (!(await confirmDialog(costs, { confirmLabel: 'Quit', danger: true }))) return false;
  }
  flushTabStorage();
  return true;
}
