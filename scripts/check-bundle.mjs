import { readdir, readFile } from 'node:fs/promises';
import { gzipSync } from 'node:zlib';

const html = await readFile('dist/index.html', 'utf8');
const entries = [
  ...new Set([...html.matchAll(/(?:src|href)="([^"\s]+\.js)"/g)].map((match) => match[1])),
];
let initial = 0;
let gzip = 0;
for (const file of await readdir('dist/assets')) {
  if (!file.endsWith('.js')) continue;
  const source = await readFile(`dist/assets/${file}`);
  if (source.length > 500_000) throw new Error(`${file} exceeds the 500 kB chunk budget`);
  if (/smoke_action|__FARO_SMOKE__/.test(source.toString()))
    throw new Error('Native test bridge leaked into production assets');
  if (entries.some((entry) => entry.endsWith(`/assets/${file}`))) {
    initial += source.length;
    gzip += gzipSync(source).length;
  }
}
if (!entries.length || initial > 500_000)
  throw new Error(`Invalid initial JavaScript budget: ${initial}`);
console.log(
  `Initial JavaScript including preloads: ${(initial / 1000).toFixed(2)} kB; ${(gzip / 1000).toFixed(2)} kB gzip. Every chunk is below 500 kB; no native test bridge is present.`,
);
