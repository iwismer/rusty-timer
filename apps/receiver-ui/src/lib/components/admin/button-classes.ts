// Compact-variant button classes moved verbatim from AdminTab.svelte.
// A later task replaces these with the shared-ui class helpers.
export const btnWarn =
  "px-2.5 py-1 text-xs font-medium rounded-md text-status-warn border border-status-warn-border bg-status-warn-bg cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
export const btnDanger =
  "px-3 py-1.5 text-xs font-medium rounded-md text-status-err border border-status-err bg-transparent cursor-pointer hover:opacity-80";
export const btnDangerConfirm =
  "px-3 py-1.5 text-xs font-medium rounded-md text-white bg-status-err border border-status-err cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
export const btnNeutral =
  "px-2.5 py-1 text-xs font-medium rounded-md text-text-primary border border-border bg-surface-2 cursor-pointer hover:opacity-80 disabled:opacity-50 disabled:cursor-not-allowed";
