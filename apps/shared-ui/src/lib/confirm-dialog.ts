export function shouldCancelOnBackdropClick(
  clickTarget: EventTarget | null,
  dialogEl: EventTarget | null,
): boolean {
  return clickTarget === dialogEl;
}

export function shouldCancelOnEscape(key: string): boolean {
  return key === "Escape";
}
