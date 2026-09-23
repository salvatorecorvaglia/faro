import { describe, expect, it } from 'vitest';
import { errorMessage, toFaroError } from '@/ipc';

/**
 * Every error path in the app renders whatever these two return, and neither
 * had a direct test — only incidental coverage through one component.
 *
 * Tauri rejects with whatever the command's error type serialized to, so the
 * tagged object is the normal case and a bare string is what a panic or a
 * non-command failure produces. Both have to survive.
 */
describe('toFaroError', () => {
  it('passes a real FaroError through untouched', () => {
    const err = { kind: 'database', message: 'relation "users" does not exist', code: '42P01' };
    expect(toFaroError(err)).toEqual(err);
  });

  it('wraps a bare string, which is what a panic comes back as', () => {
    expect(toFaroError('the backend panicked')).toEqual({
      kind: 'other',
      message: 'the backend panicked',
    });
  });

  it('wraps a thrown Error without losing its message', () => {
    expect(errorMessage(new Error('boom'))).toBe('Error: boom');
  });

  it('never produces an empty message, whatever it is handed', () => {
    // A blank banner is indistinguishable from no banner, so these must all
    // produce *something* the user can read. `String([])` and `String('')`
    // are both empty, which rendered a failure as silence.
    for (const value of [null, undefined, 0, false, {}, [], '', '   ']) {
      const message = errorMessage(value);
      expect(typeof message).toBe('string');
      expect(message.length).toBeGreaterThan(0);
    }
  });

  it('does not mistake a partial object for a FaroError', () => {
    // `kind` without `message` would render an empty banner if it were trusted.
    expect(toFaroError({ kind: 'database' })).toMatchObject({ kind: 'other' });
    expect(toFaroError({ message: 'no kind here' })).toMatchObject({ kind: 'other' });
  });
});
