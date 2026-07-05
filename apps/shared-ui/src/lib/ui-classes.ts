export type ButtonVariant = 'primary' | 'secondary' | 'warn' | 'danger' | 'danger-solid';
export type ButtonSize = 'xs' | 'sm' | 'md';

const baseButtonClasses =
  'font-medium rounded-md cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed';

const buttonSizeClasses: Record<ButtonSize, string> = {
  xs: 'px-2.5 py-1 text-xs',
  sm: 'px-3 py-1.5 text-sm',
  md: 'px-4 py-2 text-sm',
};

const buttonVariantClasses: Record<ButtonVariant, string> = {
  primary: 'text-white bg-accent border-none hover:bg-accent-hover',
  secondary: 'bg-surface-2 text-text-primary border border-border hover:bg-surface-3',
  warn: 'text-status-warn border border-status-warn-border bg-status-warn-bg hover:opacity-80',
  danger: 'text-status-err border border-status-err bg-transparent hover:opacity-80',
  'danger-solid': 'text-white bg-status-err border border-status-err hover:opacity-80',
};

export function buttonClass(
  variant: ButtonVariant = 'secondary',
  size: ButtonSize = 'sm',
  extra = ''
): string {
  return [baseButtonClasses, buttonSizeClasses[size], buttonVariantClasses[variant], extra]
    .filter(Boolean)
    .join(' ');
}

const inputBaseClass =
  'w-full px-3 py-1.5 text-sm rounded-md bg-surface-0 border border-border text-text-primary';
const inputFocusClass = 'focus:outline-none focus:ring-1 focus:ring-accent';

export const inputClass = `${inputBaseClass} ${inputFocusClass}`;
export const inputMonoClass = `${inputBaseClass} font-mono ${inputFocusClass}`;
