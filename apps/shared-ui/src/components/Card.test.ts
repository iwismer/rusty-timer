import { describe, expect, it } from 'vitest';

import cardSource from './Card.svelte?raw';

describe('Card help-only layout', () => {
  it('does not render a header row solely because help is configured', () => {
    expect(cardSource).toContain('{#if title || header}');
    expect(cardSource).not.toContain('{#if title || header || helpSection}');
  });
});
