import { useEffect, useRef } from 'react';

import { IconClose } from './icons';

export function Spinner({ size = 13 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" className="animate-spin" aria-hidden="true">
      <circle
        cx="12"
        cy="12"
        r="9"
        stroke="currentColor"
        strokeWidth="3"
        opacity="0.25"
        fill="none"
      />
      <path
        d="M21 12a9 9 0 0 0-9-9"
        stroke="currentColor"
        strokeWidth="3"
        strokeLinecap="round"
        fill="none"
      />
    </svg>
  );
}

/**
 * A modal dialog.
 *
 * Uses the native `<dialog>` element so focus trapping, Escape handling and the
 * top layer come from the platform rather than being reimplemented.
 */
export function Modal({
  open,
  onClose,
  title,
  children,
  width = 460,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  children: React.ReactNode;
  width?: number;
}) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (open && !el.open) el.showModal();
    if (!open && el.open) el.close();
  }, [open]);

  if (!open) return null;

  return (
    <dialog
      ref={ref}
      onCancel={(e) => {
        // Escape fires `cancel`; route it through onClose so React state and
        // the DOM's open attribute cannot drift apart.
        e.preventDefault();
        onClose();
      }}
      onClick={(e) => {
        // Clicking the backdrop lands on the dialog element itself.
        if (e.target === ref.current) onClose();
      }}
      className="m-auto rounded-xl p-0 backdrop:bg-black/45"
      style={{ background: 'var(--bg)', color: 'var(--text)', width, maxWidth: '92vw' }}
    >
      <div
        className="flex items-center justify-between border-b px-4 py-2.5"
        style={{ borderColor: 'var(--border)' }}
      >
        <h2 className="text-[13px] font-semibold">{title}</h2>
        <button className="btn btn-ghost -mr-1.5 px-1.5" onClick={onClose} aria-label="Close">
          <IconClose />
        </button>
      </div>
      <div className="p-4">{children}</div>
    </dialog>
  );
}

export function Field({
  label,
  children,
  hint,
}: {
  label: string;
  children: React.ReactNode;
  hint?: string;
}) {
  return (
    <label className="block">
      <span className="label">{label}</span>
      {children}
      {hint && (
        <span className="mt-1 block text-[11px]" style={{ color: 'var(--text-faint)' }}>
          {hint}
        </span>
      )}
    </label>
  );
}

/** Inline error strip, used for connection and query failures. */
export function ErrorBanner({ message, onDismiss }: { message: string; onDismiss?: () => void }) {
  return (
    <div
      className="flex items-start gap-2 rounded-md px-2.5 py-2 text-[12px]"
      style={{
        background: 'color-mix(in srgb, var(--danger) 12%, transparent)',
        color: 'var(--danger)',
      }}
      role="alert"
    >
      {/* Database errors are often multi-line; preserve their formatting. */}
      <span className="selectable min-w-0 flex-1 whitespace-pre-wrap break-words font-mono">
        {message}
      </span>
      {onDismiss && (
        <button
          onClick={onDismiss}
          className="shrink-0 opacity-70 hover:opacity-100"
          aria-label="Dismiss"
        >
          <IconClose size={13} />
        </button>
      )}
    </div>
  );
}

/** Centred placeholder for empty panes. */
export function EmptyState({
  icon,
  title,
  hint,
  action,
}: {
  icon?: React.ReactNode;
  title: string;
  hint?: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center">
      {icon && <div style={{ color: 'var(--text-faint)' }}>{icon}</div>}
      <p className="text-[13px] font-medium" style={{ color: 'var(--text-muted)' }}>
        {title}
      </p>
      {hint && (
        <p className="max-w-xs text-[12px]" style={{ color: 'var(--text-faint)' }}>
          {hint}
        </p>
      )}
      {action && <div className="mt-1">{action}</div>}
    </div>
  );
}
