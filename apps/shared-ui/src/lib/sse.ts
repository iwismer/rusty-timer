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
      let parsed: unknown;
      try {
        parsed = JSON.parse(e.data);
      } catch (err) {
        console.error(`Failed to parse SSE event "${eventName}":`, err);
        return;
      }
      try {
        handler(parsed);
      } catch (err) {
        console.error(`SSE handler error for "${eventName}":`, err);
      }
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
