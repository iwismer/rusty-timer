import type { PublicAnnouncerDelta } from "./api";

interface AnnouncerEventHandlers {
  onUpdate: (delta: PublicAnnouncerDelta) => void;
  onResync?: () => void;
}

export function connectAnnouncerEvents(
  handlers: AnnouncerEventHandlers,
): EventSource {
  const es = new EventSource("/api/v1/public/announcer/events");
  es.addEventListener("announcer_update", (event: MessageEvent) => {
    try {
      handlers.onUpdate(JSON.parse(event.data) as PublicAnnouncerDelta);
    } catch {
      // Ignore malformed event payloads; next update or resync will recover.
    }
  });
  es.addEventListener("resync", () => {
    handlers.onResync?.();
  });
  return es;
}
