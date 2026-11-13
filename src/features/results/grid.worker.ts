import type { ResultSet } from '@/ipc/types';
import { applyGridOps, type GridFilter, type SortState } from '@/lib/grid';

export interface GridRequest {
  id: number;
  result?: ResultSet;
  sort: SortState | null;
  filters: GridFilter[];
}
export interface GridReply {
  id: number;
  indices?: number[];
  error?: string;
}
let result: ResultSet | undefined;
self.onmessage = (event: MessageEvent<GridRequest>) => {
  const request = event.data;
  if (request.result) result = request.result;
  try {
    if (!result) throw new Error('No result page is loaded');
    const indices = new Map(result.rows.map((row, index) => [row, index]));
    const rows = applyGridOps(result, request.sort, request.filters).rows;
    self.postMessage({
      id: request.id,
      indices: rows.map((row) => indices.get(row)!),
    } satisfies GridReply);
  } catch (error) {
    self.postMessage({ id: request.id, error: String(error) } satisfies GridReply);
  }
};
