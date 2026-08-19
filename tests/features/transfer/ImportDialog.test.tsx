import { open as openFile } from '@tauri-apps/plugin-dialog';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { ImportDialog } from '@/features/transfer/ImportDialog';
import type { ColumnDetail, ImportPreview } from '@/ipc/types';
import { expectCallCount, mockInvoke } from '@/test/ipc';

const columns: ColumnDetail[] = [
  { name: 'id', typeName: 'int', nullable: false, default: null, isPrimaryKey: true, ordinal: 0 },
  {
    name: 'name',
    typeName: 'text',
    nullable: true,
    default: null,
    isPrimaryKey: false,
    ordinal: 1,
  },
];

const preview: ImportPreview = {
  columns: ['id', 'name'],
  sampleRows: [['1', 'Ada']],
  totalRows: 42,
  inferredTypes: ['integer', 'text'],
};

async function chooseFile() {
  await userEvent.click(screen.getByRole('button', { name: /browse/i }));
  await screen.findByText(/Column mapping/);
}

function renderDialog(onImported = () => {}) {
  vi.mocked(openFile).mockResolvedValue('/tmp/data.csv');
  mockInvoke({
    preview_import: () => preview,
    import_file: () => ({ rows: 42, path: '/tmp/data.csv' }),
  });
  return render(
    <ImportDialog
      open
      onClose={() => {}}
      connectionId="c1"
      table={{ schema: null, name: 'people' }}
      columns={columns}
      onImported={onImported}
    />,
  );
}

describe('ImportDialog', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
  });

  it('confirms before writing, naming the target and row count', async () => {
    // A wrong mapping writes plausible-looking data into the wrong columns —
    // the same reason every other irreversible write in the app (restore,
    // apply) confirms first. This regression-tests that Import does too.
    renderDialog();
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Import' }));

    expect(window.confirm).toHaveBeenCalledTimes(1);
    const message = vi.mocked(window.confirm).mock.calls[0]?.[0] as string;
    expect(message).toContain('people');
    expect(message).toContain('42');
  });

  it('does not import when the confirmation is declined', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(false);
    renderDialog();
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Import' }));

    expectCallCount('import_file', 0);
    expect(screen.queryByText(/Imported/)).not.toBeInTheDocument();
  });

  it('imports once confirmed', async () => {
    const onImported = vi.fn();
    renderDialog(onImported);
    await chooseFile();

    await userEvent.click(screen.getByRole('button', { name: 'Import' }));

    expect(await screen.findByText(/Imported 42 rows/)).toBeInTheDocument();
    expect(onImported).toHaveBeenCalledTimes(1);
  });
});
