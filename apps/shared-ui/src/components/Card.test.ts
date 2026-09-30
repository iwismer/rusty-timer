import { describe, expect, it } from 'vitest';

import cardSource from './Card.svelte?raw';

describe('Card help-only layout', () => {
  it('does not render a header row solely because help is configured', () => {
    expect(cardSource).toContain('{#if title || header}');
    expect(cardSource).not.toContain('{#if title || header || helpSection}');
  });

  it('wraps header snippet in a flex-1 container so right-aligned items stay right-aligned with help', () => {
    expect(cardSource).toContain('<div class="flex flex-1 flex-wrap items-center gap-3 min-w-0">');
    expect(cardSource).toMatch(/header\s*\?\s*''\s*:\s*'ml-auto '/);
  });
});
