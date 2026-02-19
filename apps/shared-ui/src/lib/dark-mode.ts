export function initDarkMode(): void {
  const mq = window.matchMedia("(prefers-color-scheme: dark)");
  applyTheme(mq.matches);
  mq.addEventListener("change", (e) => applyTheme(e.matches));
}

export function toggleDarkMode(): void {
  document.documentElement.classList.toggle("dark");
}

function applyTheme(dark: boolean): void {
  document.documentElement.classList.toggle("dark", dark);
}
