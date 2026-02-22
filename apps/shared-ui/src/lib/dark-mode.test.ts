import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { get } from "svelte/store";

function setupBrowserMocks(savedTheme: string | null = null): {
  colorScheme: () => string;
} {
  const style = { colorScheme: "" };

  vi.stubGlobal("document", {
    documentElement: { style },
  });

  const storage = new Map<string, string>();
  if (savedTheme !== null) {
    storage.set("rusty-timer-theme", savedTheme);
  }
  vi.stubGlobal("localStorage", {
    getItem: vi.fn((key: string) => storage.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => {
      storage.set(key, value);
    }),
  });

  return { colorScheme: () => style.colorScheme };
}

describe("initDarkMode", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.unstubAllGlobals();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("defaults to auto (empty colorScheme) when no saved preference", async () => {
    const { colorScheme } = setupBrowserMocks();
    const { initDarkMode } = await import("./dark-mode");

    initDarkMode();

    expect(colorScheme()).toBe("");
  });

  it("applies saved dark preference", async () => {
    const { colorScheme } = setupBrowserMocks("dark");
    const { initDarkMode } = await import("./dark-mode");

    initDarkMode();

    expect(colorScheme()).toBe("dark");
  });

  it("applies saved light preference", async () => {
    const { colorScheme } = setupBrowserMocks("light");
    const { initDarkMode } = await import("./dark-mode");

    initDarkMode();

    expect(colorScheme()).toBe("light");
  });

  it("treats invalid saved value as auto", async () => {
    const { colorScheme } = setupBrowserMocks("invalid");
    const { initDarkMode } = await import("./dark-mode");

    initDarkMode();

    expect(colorScheme()).toBe("");
  });

  it("cycles through auto → light → dark → auto", async () => {
    const { colorScheme } = setupBrowserMocks();
    const { initDarkMode, cycleTheme, themeMode } = await import(
      "./dark-mode"
    );

    initDarkMode();
    expect(get(themeMode)).toBe("auto");
    expect(colorScheme()).toBe("");

    cycleTheme();
    expect(get(themeMode)).toBe("light");
    expect(colorScheme()).toBe("light");

    cycleTheme();
    expect(get(themeMode)).toBe("dark");
    expect(colorScheme()).toBe("dark");

    cycleTheme();
    expect(get(themeMode)).toBe("auto");
    expect(colorScheme()).toBe("");
  });

  it("persists theme changes to localStorage", async () => {
    setupBrowserMocks();
    const { initDarkMode, cycleTheme } = await import("./dark-mode");

    initDarkMode();
    cycleTheme(); // auto → light

    expect(localStorage.setItem).toHaveBeenCalledWith(
      "rusty-timer-theme",
      "light",
    );
  });
});
