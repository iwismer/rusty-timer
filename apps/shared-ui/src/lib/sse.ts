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

  for (const [eventName, handler] of Object.entries(handlers)) {
    eventSource.addEventListener(eventName, (e: MessageEvent) => {
      handler(JSON.parse(e.data));
    });
  }

  eventSource.onopen = () => {
    onConnection?.(true);
  };

  eventSource.onerror = () => {
    onConnection?.(false);
  };

  return {
    destroy: () => eventSource.close(),
  };
}
