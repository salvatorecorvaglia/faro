import { beforeEach, describe, expect, it } from 'vitest';

import { confirmDialog, useConfirmStore } from '@/state/confirm';

beforeEach(() => useConfirmStore.setState({ request: null }));

/** Answer the currently pending request, as ConfirmHost would. */
function answer(ok: boolean) {
  const request = useConfirmStore.getState().request;
  request?.resolve(ok);
  useConfirmStore.setState({ request: null });
}

describe('confirmDialog', () => {
  it('resolves with the answer the user gives', async () => {
    const asked = confirmDialog('Delete it?');
    expect(useConfirmStore.getState().request?.message).toBe('Delete it?');
    answer(true);
    await expect(asked).resolves.toBe(true);
  });

  it('carries the label and danger styling through to the host', () => {
    void confirmDialog('Discard?', { confirmLabel: 'Discard', danger: true });
    const request = useConfirmStore.getState().request;
    expect(request?.confirmLabel).toBe('Discard');
    expect(request?.danger).toBe(true);
  });

  it('defaults the label and styling when none are given', () => {
    void confirmDialog('Sure?');
    const request = useConfirmStore.getState().request;
    expect(request?.confirmLabel).toBe('Confirm');
    expect(request?.danger).toBe(false);
  });

  it('settles a request that a second one displaces', async () => {
    // ConfirmHost only ever resolves whatever is currently in the store, so
    // overwriting a pending request used to leave its promise unsettled
    // forever — and the `await` behind it (a tab switch, say) simply never
    // continued. Reachable when a debounced guard fires mid-prompt.
    const first = confirmDialog('First?');
    const second = confirmDialog('Second?');

    // The displaced question is declined rather than abandoned.
    await expect(first).resolves.toBe(false);

    // The newer one is what is on screen, and still answerable.
    expect(useConfirmStore.getState().request?.message).toBe('Second?');
    answer(true);
    await expect(second).resolves.toBe(true);
  });

  it('leaves no pending request behind after a displaced prompt is answered', async () => {
    const first = confirmDialog('First?');
    const second = confirmDialog('Second?');
    await first;
    answer(false);
    await second;
    expect(useConfirmStore.getState().request).toBeNull();
  });
});
