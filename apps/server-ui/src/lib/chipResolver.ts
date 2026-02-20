import type { ParticipantEntry } from "./api";

export interface ChipMap {
  /** chip_id → { bib, first_name, last_name } */
  [chipId: string]: { bib: number; first_name: string; last_name: string };
}

export function buildChipMap(participants: ParticipantEntry[]): ChipMap {
  const map: ChipMap = {};
  for (const p of participants) {
    for (const chipId of p.chip_ids) {
      map[chipId] = {
        bib: p.bib,
        first_name: p.first_name,
        last_name: p.last_name,
      };
    }
  }
  return map;
}

export function resolveChipRead(
  tagId: string | null,
  readerTimestamp: string | null,
  chipMap: ChipMap | null,
): string {
  const timeStr = readerTimestamp ?? "no timestamp";

  if (!tagId) {
    return `Unparsed read — ${timeStr}`;
  }

  if (!chipMap) {
    return `Chip ${tagId} — ${timeStr}`;
  }

  const match = chipMap[tagId];
  if (match) {
    return `${match.first_name} ${match.last_name} (#${match.bib}) — ${timeStr}`;
  }

  return `Chip ${tagId} — Unknown — ${timeStr}`;
}
