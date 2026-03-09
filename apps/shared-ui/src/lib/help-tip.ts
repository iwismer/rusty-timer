/** Determine popover position: "below" by default, "above" if near viewport bottom. */
export function resolvePopoverPosition(
  buttonRect: { bottom: number; top: number },
  viewportHeight: number,
  popoverHeight: number = 200,
): "above" | "below" {
  const spaceBelow = viewportHeight - buttonRect.bottom;
  return spaceBelow < popoverHeight && buttonRect.top > popoverHeight ? "above" : "below";
}
