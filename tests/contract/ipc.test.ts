import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

/**
 * The IPC boundary is two hand-maintained lists that have to agree, and
 * nothing checked that they did.
 *
 * `lib.rs` registers the commands Tauri will answer; `src/ipc/index.ts` names
 * the ones the app calls. A rename on either side produces no compile error on
 * the other — the frontend just gets a rejected promise at runtime, in whatever
 * feature happened to call it. The same goes for `FaroError::kind()`, whose
 * discriminants the frontend's union type repeats by hand.
 *
 * Read from source rather than generated, deliberately: a generator would make
 * the two agree by construction and there would be nothing left to test, but it
 * is a larger change than this project needs. This catches the drift instead.
 */

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const read = (p: string) => readFileSync(resolve(root, p), 'utf8');

/** Command names registered with `tauri::generate_handler!`. */
function registeredCommands(): string[] {
  const source = read('src-tauri/src/lib.rs');
  const block = source.match(/generate_handler!\[([\s\S]*?)\]/);
  if (!block?.[1]) throw new Error('could not find the generate_handler! block in lib.rs');
  return [...block[1].matchAll(/commands::([a-z0-9_]+)/g)].map((m) => m[1] as string).sort();
}

/** Command names the frontend passes to `invoke`. */
function calledCommands(): string[] {
  const source = read('src/ipc/index.ts');
  return [
    ...new Set(
      [...source.matchAll(/invoke<[^>]*>\(\s*'([a-z0-9_]+)'/g)].map((m) => m[1] as string),
    ),
  ].sort();
}

describe('the IPC command list', () => {
  it('registers every command the frontend calls', () => {
    const missing = calledCommands().filter((c) => !registeredCommands().includes(c));
    expect(missing, 'called from src/ipc/index.ts but not registered in lib.rs').toEqual([]);
  });

  it('has no registered command the frontend never calls', () => {
    // A command with no caller is either dead weight or a feature that was
    // wired up on one side only — `statement_at_cursor` sat unreachable this
    // way, with three unit tests keeping it alive.
    const unused = registeredCommands().filter((c) => !calledCommands().includes(c));
    expect(unused, 'registered in lib.rs but never called from src/ipc/index.ts').toEqual([]);
  });

  it('finds a non-trivial number of commands on both sides', () => {
    // Guards the parsing itself: a regex that silently matched nothing would
    // make both assertions above pass vacuously.
    expect(registeredCommands().length).toBeGreaterThan(20);
    expect(calledCommands().length).toBeGreaterThan(20);
  });
});

describe('the FaroError kind contract', () => {
  /** Discriminants emitted by `FaroError::kind()`. */
  function rustKinds(): string[] {
    const source = read('src-tauri/src/error.rs');
    const fn = source.match(/fn kind\(&self\) -> &'static str \{([\s\S]*?)\n {4}\}/);
    if (!fn?.[1]) throw new Error('could not find FaroError::kind in error.rs');
    return [...fn[1].matchAll(/=>\s*"([a-zA-Z]+)"/g)].map((m) => m[1] as string).sort();
  }

  /** Members of the `FaroError['kind']` union. */
  function typeScriptKinds(): string[] {
    const source = read('src/ipc/types.ts');
    const decl = source.match(/export interface FaroError \{([\s\S]*?)\n\}/);
    if (!decl?.[1]) throw new Error('could not find the FaroError interface in types.ts');
    const union = decl[1].slice(0, decl[1].indexOf('message:'));
    return [...union.matchAll(/'([a-zA-Z]+)'/g)].map((m) => m[1] as string).sort();
  }

  it('matches the frontend union exactly', () => {
    // The frontend switches on these strings; a renamed variant would fall
    // through to the default branch silently rather than failing to build.
    expect(typeScriptKinds()).toEqual(rustKinds());
    expect(rustKinds().length).toBeGreaterThan(5);
  });
});
