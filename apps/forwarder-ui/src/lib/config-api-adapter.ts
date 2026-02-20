export function mapSaveSectionResult(result: { ok: boolean; error?: string }) {
  return {
    ok: result.ok,
    error: result.error,
    restart_needed: result.ok,
  };
}
