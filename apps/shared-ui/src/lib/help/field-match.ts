import type { FieldHelp } from "./help-types";

/** Check if any text field in a FieldHelp matches the given query (case-insensitive). */
export function fieldMatchesQuery(field: FieldHelp, query: string): boolean {
  const q = query.toLowerCase();
  return [field.label, field.summary, field.detailHtml, field.default, field.range, field.recommended]
    .some(text => text?.toLowerCase().includes(q));
}
