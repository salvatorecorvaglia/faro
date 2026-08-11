import { invoke } from '@tauri-apps/api/core';
import { expect, type Mock } from 'vitest';

/**
 * Helpers for driving the mocked Tauri bridge.
 *
 * Every backend call goes through `invoke(command, args)`, so routing by
 * command name lets a test say what the backend returns without caring how
 * `src/ipc/index.ts` spells the wrapper.
 */

type Handler = (args: Record<string, unknown>) => unknown;

const mock = invoke as unknown as Mock;

/**
 * Answer the named commands; anything else returns undefined.
 *
 * A handler may return a value, a promise, or throw — throwing is how you
 * simulate a backend error, which is the case most of the app's error paths
 * have never been exercised against.
 */
export function mockInvoke(handlers: Record<string, Handler>) {
  mock.mockImplementation(async (command: string, args: Record<string, unknown> = {}) => {
    const handler = handlers[command];
    if (!handler) return undefined;
    return handler(args);
  });
  return mock;
}

/** Make a command reject with a `FaroError`, the shape Tauri actually sends. */
export function faroError(kind: string, message: string) {
  return () => {
    throw { kind, message };
  };
}

/** Every `invoke` call for one command, in order. */
export function callsTo(command: string): Record<string, unknown>[] {
  return mock.mock.calls
    .filter((call: unknown[]) => call[0] === command)
    .map((call: unknown[]) => (call[1] ?? {}) as Record<string, unknown>);
}

/** Assert a command was invoked exactly `times` times. */
export function expectCallCount(command: string, times: number) {
  expect(callsTo(command)).toHaveLength(times);
}
