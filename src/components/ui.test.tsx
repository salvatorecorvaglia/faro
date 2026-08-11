import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { ErrorBanner, Field, Modal } from './ui';

describe('Modal', () => {
  it('renders nothing until it is opened', () => {
    render(
      <Modal open={false} onClose={() => {}} title="Export">
        <p>body</p>
      </Modal>,
    );
    expect(screen.queryByText('Export')).not.toBeInTheDocument();
  });

  it('shows its title and children when open', () => {
    render(
      <Modal open onClose={() => {}} title="Export">
        <p>body</p>
      </Modal>,
    );
    expect(screen.getByRole('heading', { name: 'Export' })).toBeInTheDocument();
    expect(screen.getByText('body')).toBeInTheDocument();
  });

  it('closes from the close button', async () => {
    const onClose = vi.fn();
    render(
      <Modal open onClose={onClose} title="Export">
        <p>body</p>
      </Modal>,
    );
    await userEvent.click(screen.getByRole('button', { name: 'Close' }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('routes the native cancel event through onClose', () => {
    // Escape on a <dialog> fires `cancel`, not a click. If that is not routed
    // through onClose, React state and the DOM's open attribute drift apart
    // and the dialog cannot be reopened.
    const onClose = vi.fn();
    render(
      <Modal open onClose={onClose} title="Export">
        <p>body</p>
      </Modal>,
    );
    const dialog = document.querySelector('dialog');
    dialog?.dispatchEvent(new Event('cancel', { cancelable: true, bubbles: true }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});

describe('Field', () => {
  it('labels its control and shows the hint', () => {
    render(
      <Field label="Database" hint="Optional">
        <input />
      </Field>,
    );
    expect(screen.getByText('Database')).toBeInTheDocument();
    expect(screen.getByText('Optional')).toBeInTheDocument();
  });
});

describe('ErrorBanner', () => {
  it('announces itself and preserves multi-line database messages', () => {
    render(<ErrorBanner message={'line one\nline two'} />);
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('line one');
    expect(alert).toHaveTextContent('line two');
  });

  it('can be dismissed when a handler is given', async () => {
    const onDismiss = vi.fn();
    render(<ErrorBanner message="boom" onDismiss={onDismiss} />);
    await userEvent.click(screen.getByRole('button', { name: /dismiss/i }));
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
