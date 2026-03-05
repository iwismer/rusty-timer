// @vitest-environment node
import { describe, it, expect } from "vitest";
import { downloadFile } from "./download";

describe("downloadFile", () => {
  it("throws when not in a browser environment", () => {
    expect(() => downloadFile("test.txt", "hello")).toThrow(
      "downloadFile requires a browser environment",
    );
  });
});
