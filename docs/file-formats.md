# File Formats

## Participant File (.ppl)

Plain-text CSV with no header row and no quoting. Each line is one participant.

| Column | Field | Required | Notes |
|--------|-------|----------|-------|
| 0 | Bib number | Yes | Integer |
| 1 | Last name | Yes | |
| 2 | First name | Yes | |
| 3 | Affiliation | No | Team/club name; empty string is valid |
| 4 | _(reserved)_ | No | Ignored |
| 5 | Gender | No | `M`, `F`, or anything else maps to `X` |

- Lines starting with `;` are comments and skipped.
- Empty lines are skipped.
- Encoding: UTF-8 preferred, Windows-1252 auto-detected as fallback.
- Upload via: `POST /api/v1/races/{race_id}/participants/upload` (multipart form data, first field is the file). Uploading **replaces** all participants for that race.
- Strict validation: if any row is malformed, the entire upload is rejected with 400.

Example:

```
;Race Day Participants
1,Mitchell,Patricia,Urban Stride,,F
2,Jones,Marcus,,,M
3,Smith,Alex,,,X
```

## Chip Assignment File (.bibchip)

Plain-text CSV mapping bib numbers to IPICO chip IDs. No header row required — header lines are automatically skipped (any line not starting with a digit is ignored).

| Column | Field | Required | Notes |
|--------|-------|----------|-------|
| 0 | Bib number | Yes | Integer |
| 1 | Chip ID | Yes | Hex string (e.g. `058003799177`) |

- Additional columns beyond position 1 are ignored.
- Encoding: UTF-8 preferred, Windows-1252 fallback.
- Upload via: `POST /api/v1/races/{race_id}/chips/upload` (multipart, first field). Uploading **replaces** all chip assignments.
- Strict validation: if any data row is malformed, the entire upload is rejected with 400.

Example:

```
BIB,CHIP
1,058003799177
2,058003799178
3,058003799179
```
