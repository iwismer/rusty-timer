import { writable } from "svelte/store";

export type ThemeMode = "light" | "dark" | "auto";

const STORAGE_KEY = "rusty-timer-theme";

export const themeMode = writable<ThemeMode>("auto");

let mediaQuery: MediaQueryList | null = null;
let unsubscribeThemeMode: (() => void) | null = null;
let unsubscribeSystemThemeChange: (() => void) | null = null;
let currentMode: ThemeMode = "auto";

function applyTheme(dark: boolean): void {
  document.documentElement.classList.toggle("dark", dark);
}

function applyMode(mode: ThemeMode): void {
  if (mode === "auto") {
    applyTheme(mediaQuery?.matches ?? false);
  } else {
    applyTheme(mode === "dark");
  }
}

function subscribeToSystemThemeChange(onChange: () => void): () => void {
  if (!mediaQuery) {
    return () => {};
  }

  const query = mediaQuery;
  const listener = () => onChange();

  if (typeof query.addEventListener === "function") {
    query.addEventListener("change", listener);
    return () => query.removeEventListener("change", listener);
  }

  if (typeof query.addListener === "function") {
    query.addListener(listener);
    return () => query.removeListener(listener);
  }

  return () => {};
}

export function initDarkMode(): void {
  unsubscribeThemeMode?.();
  unsubscribeSystemThemeChange?.();

  mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");

  const saved = localStorage.getItem(STORAGE_KEY) as ThemeMode | null;
  const mode = saved === "light" || saved === "dark" ? saved : "auto";
  currentMode = mode;
  themeMode.set(mode);
  applyMode(mode);

  unsubscribeSystemThemeChange = subscribeToSystemThemeChange(() => {
    if (currentMode === "auto") {
      applyTheme(mediaQuery?.matches ?? false);
    }
  });

  unsubscribeThemeMode = themeMode.subscribe((m) => {
    currentMode = m;
    localStorage.setItem(STORAGE_KEY, m);
    applyMode(m);
  });
}

export function cycleTheme(): void {
  themeMode.update((current) => {
    const order: ThemeMode[] = ["auto", "light", "dark"];
    return order[(order.indexOf(current) + 1) % order.length];
  });
}
