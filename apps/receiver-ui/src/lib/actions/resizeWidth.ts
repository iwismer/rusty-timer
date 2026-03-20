export function resizeWidth(
  node: HTMLElement,
  onWidth: (width: number) => void,
): { destroy: () => void } {
  const observer = new ResizeObserver((entries) => {
    for (const entry of entries) {
      onWidth(entry.contentRect.width);
    }
  });
  observer.observe(node);
  onWidth(node.getBoundingClientRect().width);
  return {
    destroy() {
      observer.disconnect();
    },
  };
}
