import type { FieldHelp, SectionHelp } from "./help/help-types";
import { fieldMatchesQuery } from "./help/field-match";

/** Filter section fields and tips by search query. Returns all if query is empty or whitespace-only. */
export function filterSectionContent(
  section: SectionHelp,
  query: string,
): { fields: [string, FieldHelp][]; tips: string[] } {
  const entries = Object.entries(section.fields);
  const tips = section.tips ?? [];

  if (!query.trim()) {
    return { fields: entries, tips };
  }

  return {
    fields: entries.filter(([, f]) => fieldMatchesQuery(f, query)),
    tips: tips.filter(t => t.toLowerCase().includes(query.toLowerCase())),
  };
}
