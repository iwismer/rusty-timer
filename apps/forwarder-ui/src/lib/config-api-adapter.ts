export function mapSaveSectionResult(result: { ok: boolean; error?: string; restart_needed?: boolean }) {
  return {
    ok: result.ok,
    error: result.error,
    restart_needed: result.ok && (result.restart_needed ?? true),
  };
}
