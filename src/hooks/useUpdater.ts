import { useEffect, useRef } from 'react';
import { useUpdaterStore } from '../state/updater';

export function useUpdater(): void {
  const checkForUpdates = useUpdaterStore((s) => s.checkForUpdates);
  const checkedRef = useRef(false);

  useEffect(() => {
    if (checkedRef.current) return;
    checkedRef.current = true;

    const timer = setTimeout(() => {
      checkForUpdates();
    }, 5000);

    return () => clearTimeout(timer);
  }, [checkForUpdates]);
}
