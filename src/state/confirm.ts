import { create } from 'zustand';

interface ConfirmRequest {
  message: string;
  confirmLabel: string;
  danger: boolean;
  resolve: (ok: boolean) => void;
}

interface ConfirmState {
  request: ConfirmRequest | null;
}

/**
 * Backs the single confirmation dialog `ConfirmHost` renders near the app
 * root. Not meant to be read directly outside that component — `confirmDialog`
 * below is the actual API.
 */
export const useConfirmStore = create<ConfirmState>(() => ({ request: null }));

export interface ConfirmOptions {
  /** Label for the affirmative button. Defaults to "Confirm". */
  confirmLabel?: string;
  /**
   * Styles the affirmative button as destructive rather than the default
   * accent — for delete/discard actions, as opposed to one like importing a
   * file that merely needs confirming before it runs.
   */
  danger?: boolean;
}

/**
 * Ask the user to confirm an action, through the same `Modal` every other
 * dialog in the app uses — rather than the browser's own unstyled,
 * event-loop-blocking `confirm()`, which every destructive action used to
 * reach for instead.
 *
 * A plain promise rather than a hook so it works from anywhere a `confirm()`
 * call used to: inside a component, but also inside a Zustand store action
 * like `closeTab`, which has no component of its own to hold dialog state.
 * `ConfirmHost`, mounted once near the app root, is the only thing that
 * actually renders the dialog; this just queues the request and waits.
 */
export function confirmDialog(message: string, options: ConfirmOptions = {}): Promise<boolean> {
  return new Promise((resolve) => {
    // Settle any request this one displaces.
    //
    // `ConfirmHost` only ever resolves whatever is currently in the store, so
    // overwriting a pending request left its promise permanently unsettled and
    // the `await` behind it hanging forever. That is reachable: a table tab's
    // debounced filter guard can fire while `setActive`/`closeTab` is already
    // awaiting an answer, and the tab switch would then silently never happen.
    // Declining the displaced question is the safe reading — it is the same
    // answer the user gives by dismissing it.
    useConfirmStore.getState().request?.resolve(false);

    useConfirmStore.setState({
      request: {
        message,
        confirmLabel: options.confirmLabel ?? 'Confirm',
        danger: options.danger ?? false,
        resolve,
      },
    });
  });
}
