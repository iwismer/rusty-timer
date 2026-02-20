import { describe, expect, it } from "vitest";
import { getForwarderConfigSectionRows } from "./forwarder-config-layout";

describe("getForwarderConfigSectionRows", () => {
  it("returns option-2 section groupings", () => {
    expect(getForwarderConfigSectionRows()).toEqual([
      ["general", "server"],
      ["auth", "journal"],
      ["uplink", "status_http"],
      ["readers"],
    ]);
  });
});
