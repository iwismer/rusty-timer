export type EventHandler = (data: unknown) => void;

export interface SseHandle {
  destroy: () => void;
}

export function createSSE(
  url: string,
  handlers: Record<string, EventHandler>,
  onConnection?: (connected: boolean) => void,
): SseHandle {
  const eventSource = new EventSource(url);
  let fallbackTimer: ReturnType<typeof setTimeout> | null = null;

  for (const [eventName, handler] of Object.entries(handlers)) {
    eventSource.addEventListener(eventName, (e: MessageEvent) => {
      handler(JSON.parse(e.data));
    });
  }

  eventSource.onopen = () => {
    if (fallbackTimer !== null) {
      clearTimeout(fallbackTimer);
      fallbackTimer = null;
    }
    onConnection?.(true);
  };

  eventSource.onerror = () => {
    if (eventSource.readyState === EventSource.CLOSED) {
      if (fallbackTimer !== null) {
        clearTimeout(fallbackTimer);
        fallbackTimer = null;
      }
      onConnection?.(false);
    } else if (fallbackTimer === null) {
      fallbackTimer = setTimeout(() => {
        fallbackTimer = null;
        onConnection?.(false);
      }, 10_000);
    }
  };

  return {
    destroy: () => {
      if (fallbackTimer !== null) {
        clearTimeout(fallbackTimer);
        fallbackTimer = null;
      }
      eventSource.close();
    },
  };
}
