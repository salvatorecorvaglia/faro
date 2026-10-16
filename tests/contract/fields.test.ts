import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

/**
 * Field names across the IPC boundary.
 *
 * `ipc.test.ts` checks that both sides name the same *commands*; nothing
 * checked the shapes passed through them. Every payload is a Rust struct
 * serialized with `rename_all = "camelCase"` on one side and a hand-written
 * TypeScript interface on the other, so a renamed or added field compiles on
 * both sides and then arrives as `undefined` — the UI quietly showing nothing,
 * or a request field the backend never reads.
 *
 * Read from source, like the command check, rather than generated: generating
 * the types would make this unnecessary, but needs a code generator whose
 * `i64` mapping (`bigint`) does not match how Faro deliberately sends integers.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

function rustFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return rustFiles(path);
    return path.endsWith('.rs') ? [path] : [];
  });
}

const camel = (snake: string) => snake.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());

/** struct name → serialized field names, for every camelCase-renamed struct. */
function rustStructs(): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const file of rustFiles(resolve(root, 'src-tauri/src'))) {
    const source = readFileSync(file, 'utf8');
    const structs = source.matchAll(
      /#\[serde\(rename_all = "camelCase"\)\]\s*pub struct (\w+)(?:<[^>]*>)?\s*\{([\s\S]*?)\n\}/g,
    );
    for (const [, name, body] of structs) {
      const fields: string[] = [];
      let attributes = '';
      for (const line of body!.split('\n')) {
        const trimmed = line.trim();
        if (trimmed.startsWith('#[')) {
          attributes += trimmed;
          continue;
        }
        const field = trimmed.match(/^pub (\w+):/);
        if (!field) continue;
        const renamed = attributes.match(/rename = "(\w+)"/);
        // A flattened struct's fields are checked as that struct; on the
        // TypeScript side it is an `extends`, not a field.
        if (!/flatten|skip(?!_serializing_if)/.test(attributes)) {
          fields.push(renamed ? renamed[1]! : camel(field[1]!));
        }
        attributes = '';
      }
      out.set(name!, fields.sort());
    }
  }
  return out;
}

/** interface name → its own field names (not those it extends). */
function typeScriptInterfaces(): Map<string, string[]> {
  const source = readFileSync(resolve(root, 'src/ipc/types.ts'), 'utf8');
  const out = new Map<string, string[]>();
  for (const [, name, body] of source.matchAll(
    /export interface (\w+)(?: extends [\w, ]+)? \{([\s\S]*?)\n\}/g,
  )) {
    const fields = [...body!.matchAll(/^ {2}(\w+)\??:/gm)].map((m) => m[1]!);
    out.set(name!, fields.sort());
  }
  return out;
}

describe('IPC payload field names', () => {
  const rust = rustStructs();
  const ts = typeScriptInterfaces();
  const shared = [...rust.keys()].filter((name) => ts.has(name)).sort();

  it('compares a non-trivial number of payload types', () => {
    // Guards the parsing: a regex that matched nothing would pass vacuously.
    expect(shared.length).toBeGreaterThan(20);
  });

  it.each(shared)('%s has the same fields on both sides', (name) => {
    expect(ts.get(name), `src/ipc/types.ts interface ${name}`).toEqual(rust.get(name));
  });
});
