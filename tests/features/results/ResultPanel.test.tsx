import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { ResultPanel } from '@/features/results/ResultPanel';
import type { ResultSet, StatementResult, Value } from '@/ipc/types';

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
      [int(3), text('Carol')],
    ],
    truncated: false,
    elapsedMs: 5,
    ...overrides,
  };
}

function statement(overrides: Partial<StatementResult> = {}): StatementResult {
  return {
    sql: 'select 1',
    outcome: { type: 'rows', ...makeResult() },
    error: null,
    ...overrides,
  };
}

function baseProps() {
  return {
    statements: [statement()],
    browseResult: null,
    error: null,
    activeIndex: 0,
    onActiveIndexChange: vi.fn(),
    running: false,
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('ResultPanel', () => {
  it('shows a placeholder before anything has run', () => {
    render(<ResultPanel {...baseProps()} statements={[]} clientSideSort />);
    expect(screen.getByText('No results yet')).toBeInTheDocument();
  });

  it('shows a running indicator while nothing has returned yet', () => {
    render(<ResultPanel {...baseProps()} statements={[]} running clientSideSort />);
    expect(screen.getByText('Running…')).toBeInTheDocument();
  });

  it('renders a top-level connection error instead of the grid', () => {
    render(<ResultPanel {...baseProps()} error="connection refused" clientSideSort />);
    expect(screen.getByRole('alert')).toHaveTextContent('connection refused');
    expect(screen.queryByRole('grid')).not.toBeInTheDocument();
  });

  it('renders a per-statement error instead of the grid', () => {
    render(
      <ResultPanel
        {...baseProps()}
        statements={[statement({ outcome: null, error: 'syntax error near SELEC' })]}
        clientSideSort
      />,
    );
    expect(screen.getByRole('alert')).toHaveTextContent('syntax error near SELEC');
  });

  it('shows an affected-rows summary for a non-SELECT statement, not a grid', () => {
    render(
      <ResultPanel
        {...baseProps()}
        statements={[statement({ outcome: { type: 'affected', rowsAffected: 4, elapsedMs: 2 } })]}
        clientSideSort
      />,
    );
    expect(screen.getAllByText('4 rows affected').length).toBeGreaterThan(0);
    expect(screen.queryByRole('grid')).not.toBeInTheDocument();
  });

  it('switches between statement tabs', () => {
    const onActiveIndexChange = vi.fn();
    render(
      <ResultPanel
        {...baseProps()}
        statements={[statement(), statement()]}
        onActiveIndexChange={onActiveIndexChange}
        clientSideSort
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: '2' }));
    expect(onActiveIndexChange).toHaveBeenCalledWith(1);
  });

  it('toggles between grid and JSON view', () => {
    render(<ResultPanel {...baseProps()} clientSideSort />);
    expect(screen.getByRole('grid')).toBeInTheDocument();

    fireEvent.click(screen.getByTitle('JSON view'));
    expect(screen.queryByRole('grid')).not.toBeInTheDocument();
    expect(screen.getByText(/"name": "Alice"/)).toBeInTheDocument();

    fireEvent.click(screen.getByTitle('Grid view'));
    expect(screen.getByRole('grid')).toBeInTheDocument();
  });

  it('opens the cell inspector on a cell click and closes it again', () => {
    render(<ResultPanel {...baseProps()} clientSideSort />);
    fireEvent.click(screen.getByText('Alice'));

    const closeButton = screen.getByRole('button', { name: 'Close inspector' });
    expect(closeButton).toBeInTheDocument();

    fireEvent.click(closeButton);
    expect(screen.queryByRole('button', { name: 'Close inspector' })).not.toBeInTheDocument();
  });

  describe('client-side sort/filter mode', () => {
    beforeEach(() => vi.useFakeTimers());
    afterEach(() => vi.useRealTimers());

    it('filters locally after a debounce and reports how many rows were hidden', () => {
      render(<ResultPanel {...baseProps()} clientSideSort />);
      fireEvent.click(screen.getByTitle('Column filters'));

      const nameFilter = screen.getAllByPlaceholderText('filter')[1]!;
      fireEvent.change(nameFilter, { target: { value: 'ali' } });

      // Not yet applied: the grid still shows all three rows.
      expect(screen.getByText('Bob')).toBeInTheDocument();

      act(() => {
        vi.advanceTimersByTime(250);
      });

      expect(screen.queryByText('Bob')).not.toBeInTheDocument();
      expect(screen.getByText('Alice')).toBeInTheDocument();
      expect(screen.getByText(/2 hidden by filters/)).toBeInTheDocument();
    });

    it('resets the local filter when a new query replaces the statements', () => {
      const first = [statement()];
      const { rerender } = render(
        <ResultPanel {...baseProps()} statements={first} clientSideSort />,
      );
      fireEvent.click(screen.getByTitle('Column filters'));

      const nameFilter = screen.getAllByPlaceholderText('filter')[1]!;
      fireEvent.change(nameFilter, { target: { value: 'ali' } });
      act(() => {
        vi.advanceTimersByTime(250);
      });
      expect(screen.queryByText('Bob')).not.toBeInTheDocument();

      // A fresh run produces a new statements array, even with identical SQL.
      rerender(<ResultPanel {...baseProps()} statements={[statement()]} clientSideSort />);

      expect(screen.getByText('Bob')).toBeInTheDocument();
      expect(screen.getAllByPlaceholderText('filter')[1]).toHaveValue('');
    });
  });

  describe('pushdown sort/filter mode (a table tab)', () => {
    function pushdownProps() {
      return {
        sort: null,
        onSortChange: vi.fn(),
        filters: [],
        onFiltersChange: vi.fn(),
      };
    }

    it('reports a filter change immediately, without the client-side debounce', () => {
      vi.useFakeTimers();
      const push = pushdownProps();
      render(<ResultPanel {...baseProps()} {...push} />);
      fireEvent.click(screen.getByTitle('Column filters'));

      const nameFilter = screen.getAllByPlaceholderText('filter')[1]!;
      fireEvent.change(nameFilter, { target: { value: 'ali' } });

      // No timer advance: pushdown mode has no debounce to wait out.
      expect(push.onFiltersChange).toHaveBeenCalledWith([
        { column: 'name', op: 'contains', value: 'ali' },
      ]);
      vi.useRealTimers();
    });

    it('reports a header click through onSortChange, not local state', () => {
      const push = pushdownProps();
      render(<ResultPanel {...baseProps()} {...push} />);
      fireEvent.click(screen.getByText('id'));
      expect(push.onSortChange).toHaveBeenCalledWith({ column: 'id', desc: false });
    });
  });
});
