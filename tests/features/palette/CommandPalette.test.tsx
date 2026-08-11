import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CommandPalette } from '@/features/palette/CommandPalette';
import { mockInvoke } from '@/test/ipc';

function open(onClose = () => {}) {
  mockInvoke({});
  return render(<CommandPalette open onClose={onClose} />);
}

describe('CommandPalette', () => {
  it('renders nothing while closed', () => {
    mockInvoke({});
    render(<CommandPalette open={false} onClose={() => {}} />);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('is a labelled dialog, not a bare overlay', () => {
    // It used to be a plain <div> with no role, so a screen reader announced
    // nothing and the page behind stayed reachable.
    open();
    expect(screen.getByRole('dialog', { name: 'Command palette' })).toBeInTheDocument();
  });

  it('exposes the search box as a combobox over the results list', () => {
    open();
    const input = screen.getByRole('combobox');
    expect(input).toHaveAttribute('aria-controls', 'palette-results');
    expect(input).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByRole('listbox', { name: 'Results' })).toBeInTheDocument();
  });

  it('closes on the dialog cancel event, however focus has moved', () => {
    // Escape used to be bound to the input's onKeyDown alone, so it stopped
    // working the moment focus left that input.
    const onClose = vi.fn();
    open(onClose);
    const dialog = document.querySelector('dialog');
    dialog?.dispatchEvent(new Event('cancel', { cancelable: true, bubbles: true }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('marks the active option for assistive tech', () => {
    open();
    const input = screen.getByRole('combobox');
    const active = input.getAttribute('aria-activedescendant');
    expect(active).toBeTruthy();
    const options = screen.getAllByRole('option');
    expect(options.some((o) => o.id === active)).toBe(true);
    expect(options.filter((o) => o.getAttribute('aria-selected') === 'true')).toHaveLength(1);
  });
});
