import type { FieldHelp, SectionHelp } from "./help/help-types";

/** Filter section fields and tips by search query. Returns all if query is empty. */
export function filterSectionContent(
  section: SectionHelp,
  query: string,
): { fields: [string, FieldHelp][]; tips: string[] } {
  const entries = Object.entries(section.fields);
  const tips = section.tips ?? [];

  if (!query.trim()) {
    return { fields: entries, tips };
  }

  const q = query.toLowerCase();
  return {
    fields: entries.filter(([, f]) =>
      [f.label, f.summary, f.detail, f.default, f.range, f.recommended]
        .some(text => text?.toLowerCase().includes(q))
    ),
    tips: tips.filter(t => t.toLowerCase().includes(q)),
  };
}
