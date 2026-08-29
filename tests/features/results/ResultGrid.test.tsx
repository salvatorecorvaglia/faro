import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ResultGrid } from '@/features/results/ResultGrid';
import type { ResultSet, Value } from '@/ipc/types';
import { emptyEdits } from '@/lib/edits';

const int = (n: number): Value => ({ kind: 'int', value: n });
const text = (s: string): Value => ({ kind: 'text', value: s });

function makeResult(overrides: Partial<ResultSet> = {}): ResultSet {
  return {
    columns: [
      { name: 'id', typeName: 'int4' },
      { name: 'name', typeName: 'text' },
    ],
    rows: [
      [int(1), text('Alice')],
      [int(2), text('Bob')],
    ],
    truncated: false,
    elapsedMs: 1,
    ...overrides,
  };
}

function renderGrid(overrides: Partial<Parameters<typeof ResultGrid>[0]> = {}) {
  const props = {
    result: makeResult(),
    sort: null,
    onSortChange: vi.fn(),
    filters: [],
    onFiltersChange: vi.fn(),
    showFilters: false,
    ...overrides,
  };
  const view = render(<ResultGrid {...(props as Parameters<typeof ResultGrid>[0])} />);
  return { ...view, props };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ResultGrid', () => {
  it('renders column headers and cell values', () => {
    renderGrid();
    expect(screen.getByText('id')).toBeInTheDocument();
    expect(screen.getByText('name')).toBeInTheDocument();
    expect(screen.getByText('Alice')).toBeInTheDocument();
    expect(screen.getByText('Bob')).toBeInTheDocument();
  });

  it('shows a placeholder when the result has no columns', () => {
    renderGrid({ result: makeResult({ columns: [], rows: [] }) });
    expect(screen.getByText('No columns returned')).toBeInTheDocument();
  });

  it('shows a placeholder when the result has no rows', () => {
    renderGrid({ result: makeResult({ rows: [] }) });
    expect(screen.getByText('No rows match')).toBeInTheDocument();
  });

  it('reports the next sort state when a header is clicked', () => {
    // ResultGrid is controlled — it reports intent via onSortChange rather
    // than tracking sort itself, so each click is checked against the sort
    // prop that would follow it in the real ascending → descending → off cycle.
    const unsorted = renderGrid();
    fireEvent.click(screen.getByText('id'));
    expect(unsorted.props.onSortChange).toHaveBeenCalledWith({ column: 'id', desc: false });
    unsorted.unmount();

    const ascending = renderGrid({ sort: { column: 'id', desc: false } });
    fireEvent.click(screen.getByText('id'));
    expect(ascending.props.onSortChange).toHaveBeenCalledWith({ column: 'id', desc: true });
    ascending.unmount();

    const descending = renderGrid({ sort: { column: 'id', desc: true } });
    fireEvent.click(screen.getByText('id'));
    expect(descending.props.onSortChange).toHaveBeenCalledWith(null);
  });

  it('reports a new filter when its value changes', () => {
    const { props } = renderGrid({ showFilters: true });
    const input = screen.getAllByPlaceholderText('filter')[1]!; // 'name' column
    fireEvent.change(input, { target: { value: 'ali' } });
    expect(props.onFiltersChange).toHaveBeenCalledWith([
      { column: 'name', op: 'contains', value: 'ali' },
    ]);
  });

  it('moves the selection with arrow keys and reports Enter on the selected cell', () => {
    const onCellClick = vi.fn();
    renderGrid({ onCellClick });
    const grid = screen.getByRole('grid');

    grid.focus();
    fireEvent.keyDown(grid, { key: 'ArrowDown' });
    fireEvent.keyDown(grid, { key: 'ArrowRight' });
    fireEvent.keyDown(grid, { key: 'Enter' });

    // Row 1, column 1 (0-indexed) is Bob's name cell.
    expect(onCellClick).toHaveBeenCalledWith(text('Bob'), 'name');
  });

  describe('inline cell editing', () => {
    function renderEditable() {
      const editing = {
        edits: emptyEdits(),
        onCellEdit: vi.fn(),
        onToggleDelete: vi.fn(),
        onInsertCellEdit: vi.fn(),
        onRemoveInsert: vi.fn(),
      };
      const view = renderGrid({ editing });
      return { ...view, editing };
    }

    it('commits the typed value on Enter', () => {
      const { editing } = renderEditable();
      fireEvent.doubleClick(screen.getByText('Alice'));

      const input = screen.getByRole('textbox');
      fireEvent.change(input, { target: { value: 'Alicia' } });
      fireEvent.keyDown(input, { key: 'Enter' });

      expect(editing.onCellEdit).toHaveBeenCalledOnce();
      expect(editing.onCellEdit).toHaveBeenCalledWith(0, 'name', { kind: 'text', value: 'Alicia' });
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    });

    it('writes NULL on Cmd+Backspace', () => {
      const { editing } = renderEditable();
      fireEvent.doubleClick(screen.getByText('Bob'));

      const input = screen.getByRole('textbox');
      fireEvent.keyDown(input, { key: 'Backspace', metaKey: true });

      expect(editing.onCellEdit).toHaveBeenCalledWith(1, 'name', { kind: 'null' });
    });

    it('Escape discards the draft without committing', () => {
      const { editing } = renderEditable();
      fireEvent.doubleClick(screen.getByText('Alice'));

      const input = screen.getByRole('textbox');
      fireEvent.change(input, { target: { value: 'discard me' } });
      fireEvent.keyDown(input, { key: 'Escape' });

      expect(editing.onCellEdit).not.toHaveBeenCalled();
      expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
    });

    it('a trailing blur after Escape does not resurrect the discarded value', () => {
      // Regression test: unmounting the focused input on Escape can still
      // deliver a native blur, which used to re-commit through onBlur even
      // though the user had just cancelled.
      const { editing } = renderEditable();
      fireEvent.doubleClick(screen.getByText('Alice'));

      const input = screen.getByRole('textbox');
      fireEvent.change(input, { target: { value: 'discard me' } });
      fireEvent.keyDown(input, { key: 'Escape' });
      fireEvent.blur(input); // simulates the trailing blur from unmounting a focused node

      expect(editing.onCellEdit).not.toHaveBeenCalled();
    });

    it('a trailing blur after Enter does not commit a second time', () => {
      const { editing } = renderEditable();
      fireEvent.doubleClick(screen.getByText('Alice'));

      const input = screen.getByRole('textbox');
      fireEvent.change(input, { target: { value: 'Alicia' } });
      fireEvent.keyDown(input, { key: 'Enter' });
      fireEvent.blur(input);

      expect(editing.onCellEdit).toHaveBeenCalledOnce();
    });
  });

  describe('column resize', () => {
    it('removes its window listeners when the grid unmounts mid-drag', () => {
      // Regression test: dragging a column and unmounting before releasing
      // the pointer used to leave `pointermove`/`pointerup` listeners on
      // `window` for the rest of the app's life.
      const addSpy = vi.spyOn(window, 'addEventListener');
      const removeSpy = vi.spyOn(window, 'removeEventListener');

      const { container, unmount } = renderGrid();
      const grip = container.querySelector('.cursor-col-resize');
      expect(grip).toBeTruthy();
      fireEvent.pointerDown(grip!, { clientX: 100 });

      const added = addSpy.mock.calls.filter(
        ([type]) => type === 'pointermove' || type === 'pointerup',
      );
      expect(added.length).toBe(2);

      unmount();

      for (const [type, handler] of added) {
        expect(removeSpy).toHaveBeenCalledWith(type, handler);
      }
    });

    it('does not remove listeners a second time when the drag ends normally', () => {
      const removeSpy = vi.spyOn(window, 'removeEventListener');

      const { container, unmount } = renderGrid();
      const grip = container.querySelector('.cursor-col-resize');
      fireEvent.pointerDown(grip!, { clientX: 100 });
      fireEvent.pointerUp(window);

      removeSpy.mockClear();
      unmount();

      const removedOnUnmount = removeSpy.mock.calls.filter(
        ([type]) => type === 'pointermove' || type === 'pointerup',
      );
      expect(removedOnUnmount).toHaveLength(0);
    });

    it('keeps a resized width when only the rows change', () => {
      // Client-side sorting and filtering hand the grid `{ ...result, rows }`
      // — a fresh object carrying the *same* columns array. Resetting the
      // layout on that identity threw away the user's column widths every
      // time they sorted or typed a character into a filter box.
      const result = makeResult();
      const { container, rerender } = renderGrid({ result });

      const widthOf = () =>
        (container.querySelector('.cursor-col-resize')!.parentElement as HTMLElement).style.width;

      const before = widthOf();
      const grip = container.querySelector('.cursor-col-resize');
      fireEvent.pointerDown(grip!, { clientX: 100 });
      fireEvent.pointerMove(window, { clientX: 260 });
      fireEvent.pointerUp(window);

      const resized = widthOf();
      expect(resized).not.toBe(before);

      // Re-sorted rows, same columns — as applyGridOps produces.
      rerender(
        <ResultGrid
          result={{ ...result, rows: [...result.rows].reverse() }}
          sort={{ column: 'id', desc: true }}
          onSortChange={vi.fn()}
          filters={[]}
          onFiltersChange={vi.fn()}
          showFilters={false}
        />,
      );

      expect(widthOf()).toBe(resized);
    });

    it('re-measures when the result genuinely has new columns', () => {
      const { container, rerender } = renderGrid();
      const widthOf = () =>
        (container.querySelector('.cursor-col-resize')!.parentElement as HTMLElement).style.width;

      const grip = container.querySelector('.cursor-col-resize');
      fireEvent.pointerDown(grip!, { clientX: 100 });
      fireEvent.pointerMove(window, { clientX: 300 });
      fireEvent.pointerUp(window);
      const resized = widthOf();

      // A different query: keeping the old widths would size these arbitrarily.
      rerender(
        <ResultGrid
          result={makeResult({
            columns: [
              { name: 'total', typeName: 'numeric' },
              { name: 'label', typeName: 'text' },
            ],
          })}
          sort={null}
          onSortChange={vi.fn()}
          filters={[]}
          onFiltersChange={vi.fn()}
          showFilters={false}
        />,
      );

      expect(widthOf()).not.toBe(resized);
    });
  });
});
