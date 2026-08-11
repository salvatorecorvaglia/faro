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
      // Backdrop dismissal on a native <dialog>. Escape is handled by
      // onCancel above, and the platform supplies the focus trap.
      onClick={(e) => {
        // Clicking the backdrop lands on the dialog element itself.
        if (e.target === ref.current) onClose();
      }}
      // Backdrop comes from the shared `dialog::backdrop` rule so every
      // overlay in the app uses the same scrim.
      className="m-auto rounded-xl p-0"
      style={{ background: 'var(--bg)', color: 'var(--text)', width, maxWidth: '92vw' }}
    >
      <div
        className="flex items-center justify-between border-b px-4 py-2.5"
        style={{ borderColor: 'var(--border)' }}
      >
        <h2 className="text-[13px] font-semibold">{title}</h2>
        <button
          type="button"
          className="btn btn-ghost -mr-1.5 px-1.5"
          onClick={onClose}
          aria-label="Close"
        >
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
    // The control arrives as `children` and is wrapped by this label; the
    // rule cannot see through a children prop.
    // biome-ignore lint/a11y/noLabelWithoutControl: see above
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
          type="button"
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

/**
 * Props that make a non-button element behave like one for a keyboard user.
 *
 * Faro's lists — tabs, connections, schemas, tables, saved queries, history —
 * are clickable `<div>`s rather than `<button>`s, because a button cannot
 * contain the nested hover-revealed action buttons each row needs. That is a
 * legitimate reason to avoid the element, but not a reason to be unreachable:
 * without a role, a tab stop and an activation key, none of these rows existed
 * for anyone not using a mouse.
 *
 * `role`, `tabIndex` and `onKeyDown` are written out at each site rather than
 * spread, because the lint rules that enforce them cannot see through a JSX
 * spread — and a rule that cannot see the fix is a rule that stays off.
 *
 * Five a11y rules remain off in `biome.json`, and this is the reason for four
 * of them, so the note lives here rather than in a file that cannot hold
 * comments:
 *
 * - `useSemanticElements` wants a real `<button>`. These rows cannot be one:
 *   each contains its own hover-revealed action buttons, and a button may not
 *   nest a button. `role="button"` plus the handler below is the supported
 *   alternative.
 * - `noStaticElementInteractions` and `noNoninteractiveElementInteractions`
 *   fire on the two native `<dialog>` backdrops, where Escape is handled by
 *   `onCancel` and the platform supplies the focus trap.
 * - `useFocusableInteractive` fires on the command palette's listbox options,
 *   which are correctly *not* tab stops: focus stays on the combobox input and
 *   `aria-activedescendant` names the active option.
 *
 * Worth re-checking whenever Biome's analysis improves.
 *
 * ```tsx
 * <div role="button" tabIndex={0} onKeyDown={rowActivation(onToggle).onKeyDown} onClick={onToggle}>
 * ```
 */
export function rowActivation(onActivate: () => void) {
  return {
    onKeyDown: (e: React.KeyboardEvent) => {
      // Space scrolls by default; Enter and Space are what a button responds to.
      if (e.key !== 'Enter' && e.key !== ' ') return;
      // Ignore keys bubbling up from a nested control — the row's own action
      // should not fire when someone activates the delete button inside it.
      if (e.target !== e.currentTarget) return;
      e.preventDefault();
      onActivate();
    },
  };
}
