import { useEffect, useRef } from 'react';
import { scheduleStartupUpdateCheck } from '@/lib/startup-update-check';
import { useUpdaterStore } from '../state/updater';

/**
 * Mount once at the app level: the automatic check runs five seconds after
 * startup (see `scheduleStartupUpdateCheck`).
 */
export function useUpdater(): void {
  const checkForUpdates = useUpdaterStore((s) => s.checkForUpdates);
  const scheduledRef = useRef(false);

  useEffect(() => {
    if (scheduledRef.current) return;
    scheduledRef.current = true;

    return scheduleStartupUpdateCheck(checkForUpdates, {
      disabled:
        import.meta.env.MODE === 'desktop-smoke' ||
        import.meta.env.FARO_DISABLE_UPDATE_CHECK === '1',
    });
  }, [checkForUpdates]);
}
