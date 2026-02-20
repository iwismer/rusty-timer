import { describe, expect, it } from "vitest";
import {
  fromConfig,
  toGeneralPayload,
  toReadersPayload,
  type ForwarderConfigFormState,
} from "./forwarder-config-form";

describe("fromConfig", () => {
  it("normalizes missing sections to empty form defaults", () => {
    const form = fromConfig({});
    expect(form.serverBaseUrl).toBe("");
    expect(form.readers).toEqual([]);
  });
});

describe("payload builders", () => {
  it("serializes empty display name as null", () => {
    expect(
      toGeneralPayload({
        generalDisplayName: "",
      } as ForwarderConfigFormState),
    ).toEqual({
      display_name: null,
    });
  });

  it("serializes readers fallback port as number/null", () => {
    const form = {
      readers: [
        {
          target: "127.0.0.1:10001",
          enabled: true,
          local_fallback_port: "12484",
        },
        {
          target: "127.0.0.1:10002",
          enabled: false,
          local_fallback_port: "",
        },
      ],
    } as ForwarderConfigFormState;

    expect(toReadersPayload(form)).toEqual({
      readers: [
        {
          target: "127.0.0.1:10001",
          enabled: true,
          local_fallback_port: 12484,
        },
        {
          target: "127.0.0.1:10002",
          enabled: false,
          local_fallback_port: null,
        },
      ],
    });
  });
});
