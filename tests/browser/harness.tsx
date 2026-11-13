import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import App from '@/App';
import { Modal } from '@/components/ui';
import { ResultPanel } from '@/features/results/ResultPanel';
import type { ResultSet } from '@/ipc/types';
import '@/styles.css';

const params = new URLSearchParams(location.search);
const rowCount = Number(params.get('rows') ?? 100);
const columnCount = Number(params.get('columns') ?? 4);
const result: ResultSet = {
  columns: Array.from({ length: columnCount }, (_, i) => ({
    name: `column_${i}`,
    typeName: 'text',
  })),
  rows: Array.from({ length: rowCount }, (_, row) =>
    Array.from({ length: columnCount }, (_, col) => ({
      kind: 'text' as const,
      value: col === 0 ? `Item ${(row * 7919) % rowCount}` : `r${row}c${col}`,
    })),
  ),
  truncated: false,
  elapsedMs: 0,
};
const statements = [{ sql: 'fixture', outcome: { type: 'rows' as const, ...result }, error: null }];
function DialogHarness() {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Open dialog
      </button>
      <Modal open={open} onClose={() => setOpen(false)} title="Long dialog">
        <input aria-label="First input" />
        {Array.from({ length: 80 }, (_, i) => (
          <p key={i}>Long content {i}</p>
        ))}
        <button type="button" onClick={() => setOpen(false)}>
          Done
        </button>
      </Modal>
    </>
  );
}
performance.mark('faro-harness-start');
createRoot(document.getElementById('root')!).render(
  params.get('case') === 'modal' ? (
    <DialogHarness />
  ) : params.get('case') === 'app' ? (
    <App />
  ) : (
    <ResultPanel
      statements={statements}
      browseResult={null}
      error={null}
      activeIndex={0}
      onActiveIndexChange={() => {}}
      running={false}
      clientSideSort
    />
  ),
);
