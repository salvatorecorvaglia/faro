import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { LoadBoundary } from '@/components/LoadBoundary';

function BrokenTab(): never {
  throw new Error('Chunk load failed');
}

it('keeps sibling controls usable when a deferred tab fails', () => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  render(
    <>
      <button type="button">New tab</button>
      <LoadBoundary>
        <BrokenTab />
      </LoadBoundary>
    </>,
  );
  expect(screen.getByText('This tab could not load')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'New tab' })).toBeEnabled();
});
