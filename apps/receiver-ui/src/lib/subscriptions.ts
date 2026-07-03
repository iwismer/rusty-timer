export interface ParsedPortOverride {
  value: number | null;
  error: string | null;
}

export interface SubscriptionBuildStream {
  forwarder_endpoint_id: string;
  stream_id: string;
  forwarder_id?: string | null;
  reader_ip?: string | null;
  subscribed: boolean;
  local_port: number | null;
  local_port_override?: number | null;
  event_type?: "start" | "finish";
}

export interface BuildUpdatedSubscriptionsParams {
  allStreams: SubscriptionBuildStream[];
  target: {
    forwarder_endpoint_id: string;
    stream_id: string;
    currentlySubscribed: boolean;
  };
  rawPortOverride?: string | number | null;
}

export interface BuildUpdatedSubscriptionsResult {
  subscriptions: Array<{
    forwarder_endpoint_id: string;
    stream_id: string;
    forwarder_id?: string;
    reader_ip?: string;
    local_port_override: number | null;
    event_type: "start" | "finish";
  }> | null;
  error: string | null;
}

export function parsePortOverrideInput(
  raw: string | number | null | undefined,
): ParsedPortOverride {
  const trimmed = String(raw ?? "").trim();
  if (trimmed === "") {
    return { value: null, error: null };
  }

  if (!/^\d+$/.test(trimmed)) {
    return {
      value: null,
      error: "Port override must be an integer (1-65535).",
    };
  }

  const parsed = Number.parseInt(trimmed, 10);
  if (parsed < 1 || parsed > 65535) {
    return { value: null, error: "Port override must be in range 1-65535." };
  }

  return { value: parsed, error: null };
}

function legacyMetadata(stream: SubscriptionBuildStream | undefined): {
  forwarder_id?: string;
  reader_ip?: string;
} {
  return {
    ...(stream?.forwarder_id != null
      ? { forwarder_id: stream.forwarder_id }
      : {}),
    ...(stream?.reader_ip != null ? { reader_ip: stream.reader_ip } : {}),
  };
}

export function buildAllSubscriptions(
  allStreams: SubscriptionBuildStream[],
): NonNullable<BuildUpdatedSubscriptionsResult["subscriptions"]> {
  return allStreams.map((stream) => ({
    forwarder_endpoint_id: stream.forwarder_endpoint_id,
    stream_id: stream.stream_id,
    ...legacyMetadata(stream),
    local_port_override: stream.subscribed
      ? (stream.local_port_override ?? null)
      : null,
    event_type: stream.subscribed ? (stream.event_type ?? "finish") : "finish",
  }));
}

export function buildUpdatedSubscriptions(
  params: BuildUpdatedSubscriptionsParams,
): BuildUpdatedSubscriptionsResult {
  const { allStreams, target } = params;
  const existingSubscribed = allStreams
    .filter((s) => s.subscribed)
    .map((s) => ({
      forwarder_endpoint_id: s.forwarder_endpoint_id,
      stream_id: s.stream_id,
      ...legacyMetadata(s),
      local_port_override: s.local_port_override ?? null,
      event_type: s.event_type ?? "finish",
    }));

  if (target.currentlySubscribed) {
    return {
      subscriptions: existingSubscribed.filter(
        (s) =>
          !(
            s.forwarder_endpoint_id === target.forwarder_endpoint_id &&
            s.stream_id === target.stream_id
          ),
      ),
      error: null,
    };
  }

  const parsed = parsePortOverrideInput(params.rawPortOverride);
  if (parsed.error) {
    return { subscriptions: null, error: parsed.error };
  }

  const targetStream = allStreams.find(
    (s) =>
      s.forwarder_endpoint_id === target.forwarder_endpoint_id &&
      s.stream_id === target.stream_id,
  );

  return {
    subscriptions: [
      ...existingSubscribed,
      {
        forwarder_endpoint_id: target.forwarder_endpoint_id,
        stream_id: target.stream_id,
        ...legacyMetadata(targetStream),
        local_port_override: parsed.value,
        event_type: "finish",
      },
    ],
    error: null,
  };
}
