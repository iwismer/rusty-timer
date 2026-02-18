export interface ParsedPortOverride {
  value: number | null;
  error: string | null;
}

export interface SubscriptionBuildStream {
  forwarder_id: string;
  reader_ip: string;
  subscribed: boolean;
  local_port: number | null;
}

export interface BuildUpdatedSubscriptionsParams {
  allStreams: SubscriptionBuildStream[];
  target: {
    forwarder_id: string;
    reader_ip: string;
    currentlySubscribed: boolean;
  };
  rawPortOverride?: string;
}

export interface BuildUpdatedSubscriptionsResult {
  subscriptions: Array<{
    forwarder_id: string;
    reader_ip: string;
    local_port_override: number | null;
  }> | null;
  error: string | null;
}

export function parsePortOverrideInput(
  raw: string | undefined,
): ParsedPortOverride {
  const trimmed = (raw ?? "").trim();
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

export function buildUpdatedSubscriptions(
  params: BuildUpdatedSubscriptionsParams,
): BuildUpdatedSubscriptionsResult {
  const { allStreams, target } = params;
  const existingSubscribed = allStreams
    .filter((s) => s.subscribed)
    .map((s) => ({
      forwarder_id: s.forwarder_id,
      reader_ip: s.reader_ip,
      local_port_override: s.local_port ?? null,
    }));

  if (target.currentlySubscribed) {
    return {
      subscriptions: existingSubscribed.filter(
        (s) =>
          !(
            s.forwarder_id === target.forwarder_id &&
            s.reader_ip === target.reader_ip
          ),
      ),
      error: null,
    };
  }

  const parsed = parsePortOverrideInput(params.rawPortOverride);
  if (parsed.error) {
    return { subscriptions: null, error: parsed.error };
  }

  return {
    subscriptions: [
      ...existingSubscribed,
      {
        forwarder_id: target.forwarder_id,
        reader_ip: target.reader_ip,
        local_port_override: parsed.value,
      },
    ],
    error: null,
  };
}
