import { useEffect, useMemo, useRef, useState } from 'react';
import type { ResultSet } from '@/ipc/types';
import { applyGridOps, type GridFilter, type SortState } from '@/lib/grid';
import type { GridReply, GridRequest } from './grid.worker';
import GridWorker from './grid.worker?worker&inline';

/** Large pages are sorted off the UI thread. Results retain the original cell objects. */
export function useClientGrid(
  result: ResultSet | null,
  enabled: boolean,
  sort: SortState | null,
  filters: GridFilter[],
) {
  const [computed, setComputed] = useState<{
    source: ResultSet;
    sort: SortState | null;
    filters: GridFilter[];
    result: ResultSet;
  } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const worker = useRef<Worker | null>(null);
  const loaded = useRef<ResultSet | null>(null);
  const sequence = useRef(0);
  const background =
    enabled && !!result && result.rows.length > 2000 && typeof Worker !== 'undefined';
  const needsWork =
    !!sort ||
    filters.some(
      (filter) => filter.value !== '' || filter.op === 'isNull' || filter.op === 'isNotNull',
    );

  useEffect(
    () => () => {
      worker.current?.terminate();
      worker.current = null;
      loaded.current = null;
    },
    [],
  );
  useEffect(() => {
    const id = ++sequence.current;
    setError(null);
    if (!background || !result || !needsWork) {
      worker.current?.terminate();
      worker.current = null;
      loaded.current = null;
      setComputed(null);
      return;
    }
    let active = true;
    try {
      const current = worker.current ?? new GridWorker();
      worker.current = current;
      current.onmessage = ({ data }: MessageEvent<GridReply>) => {
        if (!active || data.id !== sequence.current) return;
        if (data.error) {
          setError(data.error);
          return;
        }
        setComputed({
          source: result,
          sort,
          filters,
          result: { ...result, rows: data.indices!.map((index) => result.rows[index]!) },
        });
      };
      current.onerror = (event) => {
        if (active && id === sequence.current)
          setError(event.message || 'Could not sort this result');
      };
      current.postMessage({
        id,
        ...(loaded.current === result ? {} : { result }),
        sort,
        filters,
      } satisfies GridRequest);
      loaded.current = result;
    } catch (failure) {
      setError(String(failure));
    }
    return () => {
      active = false;
    };
  }, [result, background, needsWork, sort, filters]);

  const rows = useMemo(() => {
    if (!result || !enabled) return result;
    if (!background) return applyGridOps(result, sort, filters);
    return computed?.source === result ? computed.result : result;
  }, [result, enabled, background, computed, sort, filters]);
  const pending =
    background &&
    needsWork &&
    !error &&
    !(computed?.source === result && computed.sort === sort && computed.filters === filters);
  return { rows, pending, error };
}
