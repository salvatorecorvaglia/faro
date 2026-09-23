/**
 * Adapter over `react-resizable-panels` v4.
 *
 * v4 renamed `PanelGroup`/`PanelResizeHandle` to `Group`/`Separator` and
 * changed `direction` to `orientation`. Wrapping it here means the rest of the
 * app reads in one vocabulary, and a future rename touches only this file.
 *
 * It also guards a sharp edge: in v4 a bare number `defaultSize` means
 * **pixels**, not percent. These wrappers take percentages and convert, so a
 * `defaultSize={20}` cannot silently become a 20-pixel sidebar.
 */
import { Group, Panel as RPanel, Separator } from 'react-resizable-panels';

export function SplitGroup({
  direction,
  children,
  className,
}: {
  direction: 'horizontal' | 'vertical';
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <Group orientation={direction} className={className}>
      {children}
    </Group>
  );
}

export function SplitPanel({
  defaultSize,
  minSize,
  maxSize,
  children,
  className,
}: {
  /** Percentage of the group, 0–100. */
  defaultSize?: number;
  minSize?: number;
  maxSize?: number;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <RPanel
      className={className}
      defaultSize={pct(defaultSize)}
      minSize={pct(minSize)}
      maxSize={pct(maxSize)}
    >
      {children}
    </RPanel>
  );
}

export function SplitHandle({ direction }: { direction: 'horizontal' | 'vertical' }) {
  const vertical = direction === 'vertical';
  return (
    <Separator
      className="shrink-0 transition-colors data-[state=drag]:!bg-[var(--accent)] hover:bg-[var(--border-strong)]"
      style={{
        background: 'var(--border)',
        // A 1px hairline is the visual, but a 1px hit target is unusable, so
        // the separator is padded out with a transparent margin.
        [vertical ? 'height' : 'width']: '1px',
        [vertical ? 'borderTop' : 'borderLeft']: '3px solid transparent',
        [vertical ? 'borderBottom' : 'borderRight']: '3px solid transparent',
        backgroundClip: 'content-box',
        cursor: vertical ? 'row-resize' : 'col-resize',
      }}
    />
  );
}

const pct = (n: number | undefined) => (n === undefined ? undefined : `${n}%`);
