// Shared non-UI logic for the receiver admin surface. Used by both the
// embedded AdminTab (desktop tab shell) and the standalone /admin route via
// the section components in $lib/components/admin/.
import * as api from "$lib/api";

export function streamKey(s: {
  forwarder_endpoint_id: string;
  stream_id: string;
}): string {
  return `${s.forwarder_endpoint_id}/${s.stream_id}`;
}

export function streamLabel(stream: api.StreamEntry): string {
  return (
    stream.display_alias ??
    (stream.forwarder_id && stream.reader_ip
      ? `${stream.forwarder_id} / ${stream.reader_ip}`
      : stream.stream_id)
  );
}

export const PORT_VALIDATION_MESSAGE =
  "Port must be 1-65535 or empty to clear.";

export type PortValidation =
  | { ok: true; port: number | null }
  | { ok: false; message: string };

/** Validate a raw port-override input: 1-65535, or empty to clear. */
export function validatePortInput(raw: string): PortValidation {
  const trimmed = raw.trim();
  if (trimmed === "") return { ok: true, port: null };
  if (!/^\d+$/.test(trimmed)) {
    return { ok: false, message: PORT_VALIDATION_MESSAGE };
  }
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
    return { ok: false, message: PORT_VALIDATION_MESSAGE };
  }
  return { ok: true, port: parsed };
}

/**
 * Success message after saving a port override. Standardized on
 * `port !== null` (not truthiness) to decide between "set" and "cleared".
 */
export function portSavedFeedback(
  port: number | null,
  sub: { forwarder_id?: string; reader_ip?: string },
): string {
  return port !== null
    ? `Port override set to ${port} for ${sub.forwarder_id} / ${sub.reader_ip}.`
    : `Port override cleared for ${sub.forwarder_id} / ${sub.reader_ip}.`;
}

export type AfterMutate = (opts?: {
  forceHydrateMode?: boolean;
}) => Promise<void> | void;

export class AdminActions {
  streams = $state<api.StreamEntry[]>([]);
  subscriptions = $state<api.SubscriptionItem[]>([]);
  loading = $state(true);
  loadError = $state<string | null>(null);
  inFlightKeys = $state<Set<string>>(new Set());
  inFlightAction = $state<string | null>(null);
  // Reset Stream Data confirm-on-second-click state (destructive action).
  confirmingStreamDataKey = $state<string | null>(null);
  feedback = $state<{ message: string; ok: boolean } | null>(null);
  // Port editing state: keyed by canonical "forwarder_endpoint_id/stream_id"
  portEdits = $state<Map<string, string>>(new Map());

  #afterMutate: AfterMutate | undefined;

  constructor(options: { afterMutate?: AfterMutate } = {}) {
    this.#afterMutate = options.afterMutate;
  }

  setFeedback(message: string, ok: boolean) {
    this.feedback = { message, ok };
  }

  async loadAll() {
    this.loading = true;
    this.loadError = null;
    try {
      const [streamsResp, subsResp] = await Promise.all([
        api.getStreams(),
        api.getSubscriptions(),
      ]);
      this.streams = streamsResp.streams;
      this.subscriptions = subsResp.subscriptions;
    } catch {
      this.streams = [];
      this.subscriptions = [];
      this.loadError = "Failed to load data.";
    } finally {
      this.loading = false;
    }
  }

  // --- Cursor reset (per-stream) ---
  async resetCursor(stream: api.StreamEntry) {
    const key = streamKey(stream);
    this.inFlightKeys = new Set(this.inFlightKeys).add(key);
    this.feedback = null;
    try {
      await api.resetStreamCursor({
        forwarder_endpoint_id: stream.forwarder_endpoint_id,
        stream_id: stream.stream_id,
      });
      this.setFeedback(`Cursor reset for ${streamLabel(stream)}.`, true);
    } catch {
      this.setFeedback(
        `Failed to reset cursor for ${streamLabel(stream)}.`,
        false,
      );
    } finally {
      const next = new Set(this.inFlightKeys);
      next.delete(key);
      this.inFlightKeys = next;
    }
  }

  // --- Local stream data reset (per-stream, destructive) ---
  async resetStreamData(stream: api.StreamEntry) {
    const key = `stream-data-${streamKey(stream)}`;
    if (this.confirmingStreamDataKey !== key) {
      this.confirmingStreamDataKey = key;
      return;
    }
    this.confirmingStreamDataKey = null;
    this.inFlightKeys = new Set(this.inFlightKeys).add(key);
    this.feedback = null;
    try {
      await api.resetStreamData({
        forwarder_endpoint_id: stream.forwarder_endpoint_id,
        stream_id: stream.stream_id,
      });
      this.setFeedback(
        `Local stream data reset for ${streamLabel(stream)}.`,
        true,
      );
      await this.loadAll();
      await this.#afterMutate?.();
    } catch {
      this.setFeedback(
        `Failed to reset local stream data for ${streamLabel(stream)}.`,
        false,
      );
    } finally {
      const next = new Set(this.inFlightKeys);
      next.delete(key);
      this.inFlightKeys = next;
    }
  }

  // --- Bulk actions ---
  async bulkAction(
    action: () => Promise<{ deleted: number } | void>,
    label: string,
    actionId: string,
    afterMutateOpts?: { forceHydrateMode?: boolean },
  ) {
    this.inFlightAction = actionId;
    this.feedback = null;
    try {
      const result = await action();
      if (result && typeof result === "object" && "deleted" in result) {
        this.setFeedback(`${label}: ${result.deleted} item(s) removed.`, true);
      } else {
        this.setFeedback(`${label}: done.`, true);
      }
      await this.loadAll();
      // Also refresh any caller-owned state (e.g. the embedded tab's global
      // store) so other tabs see the changes.
      await this.#afterMutate?.(afterMutateOpts);
    } catch {
      this.setFeedback(`${label}: failed.`, false);
    } finally {
      this.inFlightAction = null;
    }
  }

  // --- Earliest epoch reset (per-stream) ---
  async resetEpoch(stream: api.StreamEntry) {
    const key = `epoch-${streamKey(stream)}`;
    this.inFlightKeys = new Set(this.inFlightKeys).add(key);
    this.feedback = null;
    try {
      await api.resetEarliestEpoch({
        forwarder_endpoint_id: stream.forwarder_endpoint_id,
        stream_id: stream.stream_id,
      });
      this.setFeedback(
        `Earliest-epoch override reset for ${streamLabel(stream)}.`,
        true,
      );
    } catch {
      this.setFeedback(
        `Failed to reset earliest-epoch for ${streamLabel(stream)}.`,
        false,
      );
    } finally {
      const next = new Set(this.inFlightKeys);
      next.delete(key);
      this.inFlightKeys = next;
    }
  }

  // --- Port override ---
  getPortDisplayValue(sub: api.SubscriptionItem): string {
    const key = streamKey(sub);
    if (this.portEdits.has(key)) return this.portEdits.get(key)!;
    return sub.local_port_override?.toString() ?? "";
  }

  handlePortInput(sub: api.SubscriptionItem, value: string) {
    const next = new Map(this.portEdits);
    next.set(streamKey(sub), value);
    this.portEdits = next;
  }

  isPortDirty(sub: api.SubscriptionItem): boolean {
    const key = streamKey(sub);
    if (!this.portEdits.has(key)) return false;
    return (
      this.portEdits.get(key)! !== (sub.local_port_override?.toString() ?? "")
    );
  }

  async savePort(sub: api.SubscriptionItem) {
    const key = streamKey(sub);
    const raw = this.portEdits.get(key) ?? "";
    const validation = validatePortInput(raw);
    if (!validation.ok) {
      this.setFeedback(validation.message, false);
      return;
    }
    const portValue = validation.port;
    const actionKey = `port-${key}`;
    this.inFlightKeys = new Set(this.inFlightKeys).add(actionKey);
    this.feedback = null;
    try {
      await api.updateLocalPort(
        {
          forwarder_endpoint_id: sub.forwarder_endpoint_id,
          stream_id: sub.stream_id,
        },
        portValue,
      );
      this.setFeedback(portSavedFeedback(portValue, sub), true);
      const next = new Map(this.portEdits);
      next.delete(key);
      this.portEdits = next;
      await this.loadAll();
    } catch {
      this.setFeedback(
        `Failed to update port for ${sub.forwarder_id} / ${sub.reader_ip}.`,
        false,
      );
    } finally {
      const next = new Set(this.inFlightKeys);
      next.delete(actionKey);
      this.inFlightKeys = next;
    }
  }
}
