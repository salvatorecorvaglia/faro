import { readFile } from 'node:fs/promises';
import { stripTypeScriptTypes } from 'node:module';
import { performance } from 'node:perf_hooks';

// Load the two pure TypeScript modules without a browser, server or extra dependency.
const source = async (path) =>
  stripTypeScriptTypes(await readFile(new URL(path, import.meta.url), 'utf8'));
const url = (code) => `data:text/javascript;base64,${Buffer.from(code).toString('base64')}`;
const value = url(await source('../src/lib/value.ts'));
const grid = (await source('../src/lib/grid.ts')).replace("'./value'", JSON.stringify(value));
const { applyGridOps } = await import(url(grid));
const result = {
  columns: [{ name: 'name', typeName: 'text' }],
  rows: Array.from({ length: 50_000 }, (_, i) => [
    { kind: 'text', value: `Item ${(i * 7919) % 50_000}` },
  ]),
  truncated: false,
  elapsedMs: 0,
};

console.log(`Node ${process.version}; 50,000 rows; median of 5 runs (fixture creation excluded)`);
for (const [name, sort, filters] of [
  ['natural text sort', { column: 'name', desc: false }, []],
  ['text filter', null, [{ column: 'name', op: 'contains', value: '123' }]],
]) {
  const times = [];
  let rows;
  for (let i = 0; i < 5; i++) {
    const start = performance.now();
    rows = applyGridOps(result, sort, filters).rows.length;
    times.push(performance.now() - start);
  }
  times.sort((a, b) => a - b);
  console.log(`${name}: ${times[2].toFixed(2)} ms; ${rows} rows`);
}
