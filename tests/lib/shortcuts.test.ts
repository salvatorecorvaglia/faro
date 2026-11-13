import { expect, it } from 'vitest';
import { shortcut } from '@/lib/shortcuts';

it('labels shortcuts for the host platform', () => {
  expect(shortcut('Enter', true, true)).toBe('⌘⇧↵');
  expect(shortcut('Enter', true, false)).toBe('Ctrl+Shift+Enter');
  expect(shortcut('S', false, false)).toBe('Ctrl+S');
});
