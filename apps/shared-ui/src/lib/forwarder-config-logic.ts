export function saveSuccessMessage(restartNeeded: boolean): string {
  return restartNeeded ? "Saved. Restart to apply." : "Saved.";
}

export function controlPowerActionsEnabled({
  persistedAllowPowerActions,
  currentAllowPowerActions,
}: {
  persistedAllowPowerActions: boolean;
  currentAllowPowerActions: boolean;
}): boolean {
  // Power actions should only be enabled when the UI checkbox matches a saved
  // enabled state. This avoids action attempts with unsaved control changes.
  return persistedAllowPowerActions && currentAllowPowerActions;
}
