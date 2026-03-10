import type { FieldHelp, SectionHelp } from "./help/help-types";
import { fieldMatchesQuery } from "./help/field-match";

/** Filter section fields and tips by search query. Returns all if query is empty or whitespace-only. */
export function filterSectionContent(
  section: SectionHelp,
  query: string,
): { fields: Array<{ fieldKey: string; field: FieldHelp }>; tips: string[] } {
  const entries = Object.entries(section.fields).map(([fieldKey, field]) => ({ fieldKey, field }));
  const tips = section.tips ?? [];

  if (!query.trim()) {
    return { fields: entries, tips };
  }

  return {
    fields: entries.filter(({ field }) => fieldMatchesQuery(field, query)),
    tips: tips.filter(t => t.toLowerCase().includes(query.toLowerCase())),
  };
}
