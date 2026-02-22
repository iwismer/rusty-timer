import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

type Listener = (event: MediaQueryListEvent) => void;

type MockMediaQueryList = MediaQueryList & {
  setMatches(next: boolean): void;
  emitChange(): void;
};

function createMediaQueryList(
  initialMatches: boolean,
  api: "modern" | "legacy" = "modern",
): MockMediaQueryList {
  let matches = initialMatches;
  const listeners = new Set<Listener>();

  const add = (listener: Listener): void => {
    listeners.add(listener);
  };

  const remove = (listener: Listener): void => {
    listeners.delete(listener);
  };

  const query = {
    media: "(prefers-color-scheme: dark)",
    onchange: null,
    get matches() {
      return matches;
    },
    dispatchEvent: vi.fn(() => true),
    setMatches(next: boolean): void {
      matches = next;
    },
    emitChange(): void {
      const event = { matches } as MediaQueryListEvent;
      for (const listener of listeners) {
        listener(event);
      }
    },
  } as MockMediaQueryList;

  if (api === "modern") {
    (query as unknown as { addEventListener: unknown }).addEventListener = vi.fn(
      (type: string, listener: Listener) => {
        if (type === "change") {
          add(listener);
        }
      },
    );
    (query as unknown as { removeEventListener: unknown })
      .removeEventListener = vi.fn((type: string, listener: Listener) => {
      if (type === "change") {
        remove(listener);
      }
    });
  } else {
    (query as unknown as { addListener: unknown }).addListener = vi.fn(
      (listener: Listener) => add(listener),
    );
    (query as unknown as { removeListener: unknown }).removeListener = vi.fn(
      (listener: Listener) => remove(listener),
    );
  }

  return query;
}

function setupBrowserMocks(mediaQuery: MockMediaQueryList): { isDark: () => boolean } {
  let dark = false;

  vi.stubGlobal("window", {
    matchMedia: vi.fn(() => mediaQuery),
  });

  vi.stubGlobal("document", {
    documentElement: {
      classList: {
        toggle: vi.fn((name: string, enabled: boolean) => {
          if (name === "dark") {
            dark = enabled;
          }
        }),
      },
    },
  });

  const storage = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: vi.fn((key: string) => storage.get(key) ?? null),
    setItem: vi.fn((key: string, value: string) => {
      storage.set(key, value);
    }),
  });

  return { isDark: () => dark };
}

describe("initDarkMode", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.unstubAllGlobals();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("applies system dark mode when theme mode is auto", async () => {
    const mediaQuery = createMediaQueryList(true, "modern");
    const { isDark } = setupBrowserMocks(mediaQuery);
    const { initDarkMode } = await import("./dark-mode");

    initDarkMode();

    expect(isDark()).toBe(true);
  });

  it("updates theme on system color scheme changes", async () => {
    const mediaQuery = createMediaQueryList(false, "modern");
    const { isDark } = setupBrowserMocks(mediaQuery);
    const { initDarkMode } = await import("./dark-mode");

    initDarkMode();
    expect(isDark()).toBe(false);

    mediaQuery.setMatches(true);
    mediaQuery.emitChange();
    expect(isDark()).toBe(true);
  });

  it("supports legacy MediaQueryList listener APIs", async () => {
    const mediaQuery = createMediaQueryList(false, "legacy");
    const { isDark } = setupBrowserMocks(mediaQuery);
    const { initDarkMode } = await import("./dark-mode");

    expect(() => initDarkMode()).not.toThrow();
    expect(isDark()).toBe(false);

    mediaQuery.setMatches(true);
    mediaQuery.emitChange();
    expect(isDark()).toBe(true);
  });
});
