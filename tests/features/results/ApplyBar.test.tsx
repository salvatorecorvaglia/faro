import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ConfirmHost } from '@/components/ConfirmDialog';
import { ApplyBar } from '@/features/results/ApplyBar';
import { useConfirmStore } from '@/state/confirm';

const noop = () => {};

function renderBar(overrides: Partial<Parameters<typeof ApplyBar>[0]> = {}) {
  const props = {
    changeCount: 2,
    applying: false,
    error: null,
    onApply: vi.fn(),
    onDiscard: vi.fn(),
    onPreview: vi.fn(async () => []),
    onDismissError: noop,
    ...overrides,
  };
  render(
    <>
      <ApplyBar {...(props as Parameters<typeof ApplyBar>[0])} />
      <ConfirmHost />
    </>,
  );
  return props;
}

beforeEach(() => {
  useConfirmStore.setState({ request: null });
});

describe('ApplyBar', () => {
  it('confirms before writing to the database', async () => {
    // Applying is irreversible and the SQL preview is opt-in, so this is the
    // only thing standing between a stray click and a committed write.
    const props = renderBar({ changeCount: 3 });
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));

    const dialog = (await screen.findByText(/3 changes/)).closest('dialog')!;
    expect(dialog).toHaveTextContent('cannot be undone');
    expect(props.onApply).not.toHaveBeenCalled();

    await userEvent.click(within(dialog).getByRole('button', { name: 'Apply' }));
    expect(props.onApply).toHaveBeenCalledOnce();
  });

  it('does not apply when the confirmation is declined', async () => {
    const props = renderBar();
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));

    const dialog = (await screen.findByText(/2 changes/)).closest('dialog')!;
    await userEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));

    expect(props.onApply).not.toHaveBeenCalled();
  });

  it('renders a rejected preview as its message, not [object Object]', async () => {
    // Tauri rejects with the serialized FaroError — a plain object, not an
    // Error — so `String(e)` produced "[object Object]".
    renderBar({
      onPreview: vi.fn(async () => {
        throw { kind: 'database', message: 'relation "t" does not exist' };
      }),
    });

    await userEvent.click(screen.getByRole('button', { name: 'Show SQL' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('relation "t" does not exist');
    expect(alert).not.toHaveTextContent('[object Object]');
  });

  it('pluralizes the change count', () => {
    renderBar({ changeCount: 1 });
    expect(screen.getByText('1 unsaved change')).toBeInTheDocument();
  });
});
