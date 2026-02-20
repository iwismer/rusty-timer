export function saveSuccessMessage(restartNeeded: boolean): string {
  return restartNeeded ? "Saved. Restart to apply." : "Saved.";
}
