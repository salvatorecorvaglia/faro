import { Modal } from '@/components/ui';
import { useConfirmStore } from '@/state/confirm';

/**
 * Renders whatever `confirmDialog()` last queued. Mounted once near the app
 * root — every call site just awaits the promise it returns.
 */
export function ConfirmHost() {
  const request = useConfirmStore((s) => s.request);

  function answer(ok: boolean) {
    request?.resolve(ok);
    useConfirmStore.setState({ request: null });
  }

  return (
    <Modal open={!!request} onClose={() => answer(false)} title="Confirm" width={400}>
      {request && (
        <div className="flex flex-col gap-3">
          <p className="whitespace-pre-line text-[12.5px]">{request.message}</p>
          <div className="flex justify-end gap-2">
            <button className="btn btn-ghost" onClick={() => answer(false)} type="button">
              Cancel
            </button>
            <button
              className="btn btn-primary"
              onClick={() => answer(true)}
              type="button"
              style={request.danger ? { background: 'var(--danger)', color: '#fff' } : undefined}
            >
              {request.confirmLabel}
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}
