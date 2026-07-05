import { writable } from "svelte/store";

export type ThemeMode = "light" | "dark" | "auto";

const STORAGE_KEY = "rusty-timer-theme";

export const themeMode = writable<ThemeMode>("auto");

function applyMode(mode: ThemeMode): void {
  // "" clears the inline style, falling back to `color-scheme: light dark` from CSS = auto
  document.documentElement.style.colorScheme = mode === "auto" ? "" : mode;
}

export function initDarkMode(): void {
  const saved = localStorage.getItem(STORAGE_KEY) as ThemeMode | null;
  const mode = saved === "light" || saved === "dark" ? saved : "auto";
  themeMode.set(mode);
  applyMode(mode);

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
