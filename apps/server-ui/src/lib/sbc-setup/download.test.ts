import { describe, expect, it, vi } from "vitest";
import { downloadFile } from "./download";

describe("downloadFile", () => {
  it("creates a text blob and clicks a temporary anchor", () => {
    const click = vi.fn();
    const remove = vi.fn();
    const appendChild = vi.fn();
    const createObjectURL = vi.fn(() => "blob:test");
    const revokeObjectURL = vi.fn();
    const anchor = {
      href: "",
      download: "",
      style: { display: "" },
      click,
      remove,
    };

    vi.stubGlobal(
      "Blob",
      class MockBlob {
        parts: unknown[];
        options: unknown;
        constructor(parts: unknown[], options: unknown) {
          this.parts = parts;
          this.options = options;
        }
      },
    );
    vi.stubGlobal("URL", { createObjectURL, revokeObjectURL });
    vi.stubGlobal("document", {
      createElement: vi.fn(() => anchor),
      body: { appendChild },
    });
    vi.useFakeTimers();

    downloadFile("user-data", "content");

    expect(createObjectURL).toHaveBeenCalledOnce();
    expect(anchor.href).toBe("blob:test");
    expect(anchor.download).toBe("user-data");
    expect(appendChild).toHaveBeenCalledWith(anchor);
    expect(click).toHaveBeenCalledOnce();
    expect(remove).toHaveBeenCalledOnce();

    vi.runAllTimers();
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:test");
    vi.useRealTimers();
  });
});
