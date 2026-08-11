import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import type { TableInfo } from '@/ipc/types';
import { mockInvoke } from '@/test/ipc';
import { BackupDialog } from './BackupDialog';

const table = (name: string): TableInfo => ({
  schema: null,
  name,
  kind: 'table',
  estimatedRows: null,
});

function renderDialog(tables: TableInfo[]) {
  mockInvoke({});
  return render(
    <BackupDialog
      open
      onClose={() => {}}
      connectionId="c1"
      connectionName="prod"
      tables={tables}
    />,
  );
}

describe('BackupDialog', () => {
  it('selects every table when the list is already loaded', () => {
    renderDialog([table('authors'), table('books')]);
    expect(screen.getByText('Tables — 2 of 2')).toBeInTheDocument();
  });

  it('selects the tables that arrive after it opens', async () => {
    // The sidebar fetches the table list *after* setting the dialog open, so
    // the first render always has an empty list. Seeding only on `open` left
    // the dialog reading "0 of N" with its primary button disabled — the
    // documented "tables default to all-selected" never happened in practice.
    const { rerender } = renderDialog([]);
    expect(screen.getByText('Tables — 0 of 0')).toBeInTheDocument();

    rerender(
      <BackupDialog
        open
        onClose={() => {}}
        connectionId="c1"
        connectionName="prod"
        tables={[table('authors'), table('books')]}
      />,
    );

    expect(await screen.findByText('Tables — 2 of 2')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /choose file/i })).toBeEnabled();
  });

  it('does not re-tick tables the user has deliberately unticked', async () => {
    // The reason seeding was tied to `open` in the first place: re-seeding on
    // every change to the list would fight the user.
    const tables = [table('authors'), table('books')];
    const { rerender } = renderDialog(tables);

    await userEvent.click(screen.getByRole('button', { name: 'None' }));
    expect(screen.getByText('Tables — 0 of 2')).toBeInTheDocument();

    // A re-render with an equivalent list must not undo that.
    rerender(
      <BackupDialog
        open
        onClose={() => {}}
        connectionId="c1"
        connectionName="prod"
        tables={[table('authors'), table('books')]}
      />,
    );
    expect(screen.getByText('Tables — 0 of 2')).toBeInTheDocument();
  });

  it('ignores views, which hold no data of their own', () => {
    renderDialog([table('authors'), { ...table('v_recent'), kind: 'view' }]);
    expect(screen.getByText('Tables — 1 of 1')).toBeInTheDocument();
  });
});
