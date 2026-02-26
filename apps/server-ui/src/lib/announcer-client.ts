import type { AnnouncerDelta } from "./api";

interface AnnouncerEventHandlers {
  onUpdate: (delta: AnnouncerDelta) => void;
  onResync?: () => void;
}

export function connectAnnouncerEvents(
  handlers: AnnouncerEventHandlers,
): EventSource {
  const es = new EventSource("/api/v1/announcer/events");
  es.addEventListener("announcer_update", (event: MessageEvent) => {
    try {
      handlers.onUpdate(JSON.parse(event.data) as AnnouncerDelta);
    } catch {
      // Ignore malformed event payloads; next update or resync will recover.
    }
  });
  es.addEventListener("resync", () => {
    handlers.onResync?.();
  });
  return es;
}
