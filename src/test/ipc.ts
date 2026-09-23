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
 * Answer the named commands; anything else is a test failure.
 *
 * A handler may return a value, a promise, or throw — throwing is how you
 * simulate a backend error, which is the case most of the app's error paths
 * have never been exercised against.
 *
 * Rejecting an undeclared command is the point. Returning `undefined` meant a
 * typo here, or a rename on either side of the IPC boundary, produced a value
 * the component would happily carry — so the test passed while exercising
 * nothing, or crashed somewhere unrelated on `.length`. Either way the failure
 * pointed away from the cause.
 *
 * A test that genuinely does not care what a command returns says so by
 * naming it: `mockInvoke({ list_tables: () => [] })`.
 */
export function mockInvoke(handlers: Record<string, Handler>) {
  mock.mockImplementation(async (command: string, args: Record<string, unknown> = {}) => {
    const handler = handlers[command];
    if (!handler) {
      throw new Error(
        `Unmocked command "${command}". Add it to this test's mockInvoke({ ... }) ` +
          `— declared: ${Object.keys(handlers).join(', ') || '(none)'}`,
      );
    }
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
