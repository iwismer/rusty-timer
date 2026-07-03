import type { StreamEntry } from "./api";

export type StreamDisplayStatus =
  | "not_subscribed"
  | "subscribing"
  | "receiving"
  | "receiving_pending"
  | "receiving_reader_down"
  | "waiting_for_data"
  | "not_receiving";

export interface StreamDisplayInputs {
  recentActivity: boolean;
  optimisticSubscribing: boolean;
}

export function deriveStreamDisplayStatus(
  stream: Pick<StreamEntry, "subscribed" | "online" | "reader_connected">,
  inputs: StreamDisplayInputs,
): StreamDisplayStatus {
  if (!stream.subscribed) return "not_subscribed";

  if (inputs.recentActivity) {
    if (stream.reader_connected === false) return "receiving_reader_down";
    if (stream.reader_connected === true) return "receiving";
    return "receiving_pending";
  }

  if (inputs.optimisticSubscribing) return "subscribing";

  if (stream.online === true) {
    if (stream.reader_connected === false) return "receiving_reader_down";
    if (stream.reader_connected === true) return "receiving";
    return "waiting_for_data";
  }

  if (stream.online == null) return "subscribing";
  return "not_receiving";
}

export function streamDisplayDotClass(status: StreamDisplayStatus): string {
  switch (status) {
    case "not_subscribed":
      return "bg-text-muted";
    case "receiving":
    case "receiving_pending":
      return "bg-status-ok";
    case "subscribing":
    case "receiving_reader_down":
    case "waiting_for_data":
      return "bg-status-warn";
    case "not_receiving":
      return "bg-status-err";
  }
}

export function streamDisplayBadge(status: StreamDisplayStatus): string | null {
  switch (status) {
    case "not_subscribed":
    case "receiving":
      return null;
    case "subscribing":
      return "Subscribing…";
    case "receiving_pending":
      return "Reader status pending";
    case "receiving_reader_down":
      return "Reader down";
    case "waiting_for_data":
      return "Waiting for data";
    case "not_receiving":
      return "Not receiving";
  }
}

export function streamDisplayLabel(status: StreamDisplayStatus): string {
  switch (status) {
    case "not_subscribed":
      return "Not subscribed";
    case "subscribing":
      return "Subscribed — connecting";
    case "receiving":
      return "Receiving";
    case "receiving_pending":
      return "Receiving — reader status pending";
    case "receiving_reader_down":
      return "Receiving — reader down";
    case "waiting_for_data":
      return "Connected — waiting for data";
    case "not_receiving":
      return "Not receiving";
  }
}
