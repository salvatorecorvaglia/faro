import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';

import { ConfirmHost } from '@/components/ConfirmDialog';
import { confirmDialog, useConfirmStore } from '@/state/confirm';

beforeEach(() => {
  useConfirmStore.setState({ request: null });
});

describe('confirmDialog / ConfirmHost', () => {
  it('resolves true when the affirmative button is clicked', async () => {
    render(<ConfirmHost />);
    const result = confirmDialog('Delete this?');

    await userEvent.click(await screen.findByRole('button', { name: 'Confirm' }));

    expect(await result).toBe(true);
    expect(screen.queryByText('Delete this?')).not.toBeInTheDocument();
  });

  it('resolves false when Cancel is clicked', async () => {
    render(<ConfirmHost />);
    const result = confirmDialog('Delete this?');

    await userEvent.click(await screen.findByRole('button', { name: 'Cancel' }));

    expect(await result).toBe(false);
  });

  it('resolves false when dismissed via Escape, like closing the dialog', async () => {
    render(<ConfirmHost />);
    const result = confirmDialog('Delete this?');

    const dialog = (await screen.findByText('Delete this?')).closest('dialog')!;
    dialog.dispatchEvent(new Event('cancel', { cancelable: true }));

    expect(await result).toBe(false);
  });

  it('uses the custom label and message supplied', async () => {
    render(<ConfirmHost />);
    confirmDialog('Discard 3 unsaved rows?', { confirmLabel: 'Discard' });

    expect(await screen.findByText('Discard 3 unsaved rows?')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Discard' })).toBeInTheDocument();
  });
});
