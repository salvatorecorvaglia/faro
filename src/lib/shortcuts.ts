/** Display the modifier used by CodeMirror's Mod bindings. */
export const IS_APPLE =
  typeof navigator !== 'undefined' &&
  /Mac|iPhone|iPad/.test(
    (navigator as Navigator & { userAgentData?: { platform?: string } }).userAgentData?.platform ??
      navigator.platform,
  );

export function shortcut(key: string, shift = false, apple = IS_APPLE): string {
  const label = apple ? ({ Enter: '↵', Backspace: '⌫' }[key] ?? key) : key;
  return apple ? `⌘${shift ? '⇧' : ''}${label}` : `Ctrl+${shift ? 'Shift+' : ''}${label}`;
}
