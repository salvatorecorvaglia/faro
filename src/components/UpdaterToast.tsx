import { useUpdaterStore } from '../state/updater';
import { IconClose, IconDownload, IconRefresh } from './icons';
import { Spinner } from './ui';

export function UpdaterToast() {
  const status = useUpdaterStore((s) => s.status);
  const version = useUpdaterStore((s) => s.version);
  const progress = useUpdaterStore((s) => s.progress);
  const error = useUpdaterStore((s) => s.error);
  const dismissed = useUpdaterStore((s) => s.dismissed);

  const downloadAndInstall = useUpdaterStore((s) => s.downloadAndInstall);
  const restartApp = useUpdaterStore((s) => s.restartApp);
  const dismiss = useUpdaterStore((s) => s.dismiss);

  if (dismissed || status === 'idle' || status === 'checking') {
    return null;
  }

  return (
    <div
      className="toast-in fixed bottom-4 right-4 z-50 flex max-w-sm flex-col gap-2 rounded-xl border p-3.5 backdrop-blur-md transition-all"
      style={{
        background: 'var(--bg)',
        borderColor: 'var(--border-strong)',
        color: 'var(--text)',
        boxShadow: 'var(--shadow-modal)',
      }}
      role="alert"
    >
      <div className="flex items-start justify-between gap-3">
        <div className="flex items-center gap-2">
          {status === 'downloading' ? (
            <Spinner size={16} />
          ) : (
            <IconDownload size={16} style={{ color: 'var(--accent)' }} />
          )}
          <span className="text-[13px] font-semibold">
            {status === 'available' && `Update v${version} available`}
            {status === 'downloading' && `Downloading update… (${progress}%)`}
            {status === 'ready' && `Update ready to install`}
            {status === 'error' && `Update error`}
          </span>
        </div>
        <button
          type="button"
          onClick={dismiss}
          className="btn btn-ghost -mr-1 p-1 opacity-70 hover:opacity-100"
          aria-label="Close notification"
        >
          <IconClose size={13} />
        </button>
      </div>

      <p className="text-[12px]" style={{ color: 'var(--text-muted)' }}>
        {status === 'available' && 'A new version of Faro is ready to download.'}
        {status === 'downloading' && `Downloading update files... ${progress}%`}
        {status === 'ready' && 'Faro will update when you restart the application.'}
        {status === 'error' && (error || 'An error occurred during update.')}
      </p>

      {status === 'downloading' && (
        <div
          className="h-1.5 w-full overflow-hidden rounded-full"
          style={{ background: 'var(--bg-inset)' }}
        >
          <div
            className="h-full transition-all duration-200"
            style={{
              width: `${progress}%`,
              background: 'var(--accent)',
            }}
          />
        </div>
      )}

      <div className="flex items-center justify-end gap-2 mt-1">
        {status === 'available' && (
          <button type="button" onClick={downloadAndInstall} className="btn btn-primary">
            <IconDownload size={13} /> Download
          </button>
        )}

        {status === 'ready' && (
          <button type="button" onClick={restartApp} className="btn btn-primary">
            <IconRefresh size={13} /> Restart now
          </button>
        )}

        {status === 'error' && (
          <button type="button" onClick={dismiss} className="btn btn-outline">
            Dismiss
          </button>
        )}
      </div>
    </div>
  );
}
