import { writable } from "svelte/store";

export type ThemeMode = "light" | "dark" | "auto";

const STORAGE_KEY = "rusty-timer-theme";

export const themeMode = writable<ThemeMode>("auto");

let mediaQuery: MediaQueryList | null = null;

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

export function initDarkMode(): void {
  mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");

  const saved = localStorage.getItem(STORAGE_KEY) as ThemeMode | null;
  const mode = saved === "light" || saved === "dark" ? saved : "auto";
  themeMode.set(mode);
  applyMode(mode);

  mediaQuery.addEventListener("change", () => {
    let current: ThemeMode = "auto";
    themeMode.subscribe((m) => (current = m))();
    if (current === "auto") {
      applyTheme(mediaQuery?.matches ?? false);
    }
  });

  themeMode.subscribe((m) => {
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
