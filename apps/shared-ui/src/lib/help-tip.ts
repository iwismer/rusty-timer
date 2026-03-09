const POPOVER_HEIGHT = 200;
const POPOVER_WIDTH = 288; // w-72 = 18rem = 288px
const GAP = 8;

/** Compute fixed-position inline style for the popover based on the trigger button's rect. */
export function computePopoverStyle(
  buttonRect: { top: number; bottom: number; left: number; right: number },
  viewportWidth: number,
  viewportHeight: number,
  popoverHeight: number = POPOVER_HEIGHT,
): string {
  const spaceBelow = viewportHeight - buttonRect.bottom;
  const showAbove = spaceBelow < popoverHeight + GAP && buttonRect.top > popoverHeight + GAP;

  const top = showAbove
    ? buttonRect.top - popoverHeight - GAP
    : buttonRect.bottom + GAP;

  // Align left edge with button, but clamp so it doesn't overflow the viewport
  let left = buttonRect.left;
  if (left + POPOVER_WIDTH > viewportWidth - GAP) {
    left = viewportWidth - POPOVER_WIDTH - GAP;
  }
  if (left < GAP) {
    left = GAP;
  }

  return `top: ${top}px; left: ${left}px;`;
}

/** @deprecated Use computePopoverStyle instead. */
export function resolvePopoverPosition(
  buttonRect: { bottom: number; top: number },
  viewportHeight: number,
  popoverHeight: number = POPOVER_HEIGHT,
): "above" | "below" {
  const spaceBelow = viewportHeight - buttonRect.bottom;
  return spaceBelow < popoverHeight && buttonRect.top > popoverHeight ? "above" : "below";
}
