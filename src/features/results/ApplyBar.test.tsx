import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ApplyBar } from './ApplyBar';

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
  render(<ApplyBar {...(props as Parameters<typeof ApplyBar>[0])} />);
  return props;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('ApplyBar', () => {
  it('confirms before writing to the database', async () => {
    // Applying is irreversible and the SQL preview is opt-in, so this is the
    // only thing standing between a stray click and a committed write.
    const confirm = vi.fn((_message?: string) => true);
    vi.stubGlobal('confirm', confirm);

    const props = renderBar({ changeCount: 3 });
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));

    expect(confirm).toHaveBeenCalledOnce();
    expect(confirm.mock.calls[0]?.[0]).toContain('3 changes');
    expect(props.onApply).toHaveBeenCalledOnce();
  });

  it('does not apply when the confirmation is declined', async () => {
    vi.stubGlobal(
      'confirm',
      vi.fn(() => false),
    );

    const props = renderBar();
    await userEvent.click(screen.getByRole('button', { name: 'Apply' }));

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
