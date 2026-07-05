import { describe, expect, it } from 'vitest';
import { buttonClass, inputClass, inputMonoClass } from './ui-classes';

const baseButtonClasses =
  'font-medium rounded-md cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed';

const sizeClasses = {
  xs: 'px-2.5 py-1 text-xs',
  sm: 'px-3 py-1.5 text-sm',
  md: 'px-4 py-2 text-sm',
};

const variantClasses = {
  primary: 'text-white bg-accent border-none hover:bg-accent-hover',
  secondary: 'bg-surface-2 text-text-primary border border-border hover:bg-surface-3',
  warn: 'text-status-warn border border-status-warn-border bg-status-warn-bg hover:opacity-80',
  danger: 'text-status-err border border-status-err bg-transparent hover:opacity-80',
  'danger-solid': 'text-white bg-status-err border border-status-err hover:opacity-80',
};

function expectTokens(className: string, expected: string) {
  expect(className.split(' ')).toEqual(expect.arrayContaining(expected.split(' ')));
}

describe('buttonClass', () => {
  it('defaults to a secondary small button', () => {
    expect(buttonClass()).toBe(
      `${baseButtonClasses} ${sizeClasses.sm} ${variantClasses.secondary}`
    );
  });

  for (const [size, classes] of Object.entries(sizeClasses)) {
    it(`includes ${size} size classes`, () => {
      expectTokens(buttonClass('secondary', size as keyof typeof sizeClasses), classes);
    });
  }

  for (const [variant, classes] of Object.entries(variantClasses)) {
    it(`includes ${variant} variant classes`, () => {
      expectTokens(buttonClass(variant as keyof typeof variantClasses, 'sm'), classes);
    });
  }

  it('includes base button classes for every button', () => {
    expectTokens(buttonClass('primary', 'md'), baseButtonClasses);
  });

  it('appends extra classes when provided', () => {
    expect(buttonClass('primary', 'xs', 'mt-2 w-full')).toBe(
      `${baseButtonClasses} ${sizeClasses.xs} ${variantClasses.primary} mt-2 w-full`
    );
  });

  it('does not append empty extra classes', () => {
    expect(buttonClass('primary', 'xs', '')).toBe(
      `${baseButtonClasses} ${sizeClasses.xs} ${variantClasses.primary}`
    );
  });
});

describe('input classes', () => {
  it('provides the standard input class without monospace styling', () => {
    expect(inputClass).toBe(
      'w-full px-3 py-1.5 text-sm rounded-md bg-surface-0 border border-border text-text-primary focus:outline-none focus:ring-1 focus:ring-accent'
    );
    expect(inputClass.split(' ')).not.toContain('font-mono');
  });

  it('provides the monospace input class', () => {
    expect(inputMonoClass).toBe(
      'w-full px-3 py-1.5 text-sm rounded-md bg-surface-0 border border-border text-text-primary font-mono focus:outline-none focus:ring-1 focus:ring-accent'
    );
  });
});
