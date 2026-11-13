import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';

function luminance(hex: string) {
  const channels = hex.match(/[a-f0-9]{2}/gi)!.map((part) => {
    const value = Number.parseInt(part, 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722;
}

it('keeps secondary text, NULL badges and comments readable on both themes', () => {
  const css = readFileSync('src/styles.css', 'utf8');
  for (const selector of [':root', ':root.dark']) {
    const block = css.slice(css.indexOf(`${selector} {`)).split('}')[0]!;
    const color = (token: string) => block.match(new RegExp(`--${token}: (#[a-f0-9]{6})`))![1]!;
    for (const text of ['text-muted', 'text-faint', 'null', 'syntax-comment']) {
      for (const background of ['bg', 'bg-subtle', 'bg-inset', 'row-alt', 'selection']) {
        const a = luminance(color(text));
        const b = luminance(color(background));
        const ratio = (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
        expect(ratio, `${selector}: ${text} on ${background}`).toBeGreaterThanOrEqual(4.5);
      }
    }
  }
});
