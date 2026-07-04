# Spec: Import participant + chip data from Race Director DBF files

Status: **Draft for review** (uncommitted)
Owner: receiver
Related: `docs/race-director/ipico-direct-dbf-format.md` (the reverse direction — we already *write* `IPICO.DBF`)

## 1. Goal

Let the receiver resolve chip reads to **bib / name / division** by reading
participant and chip-assignment data directly from a Race Director (RD) data
directory, instead of manually exporting and importing `.ppl` / `.bibchip`
files.

We already emit `IPICO.DBF` for RD to consume; this is the same integration in
reverse: RD's own DBFs become an *import source*.

### Non-goals (this spec)

- Gun time / finish time / net time. Explicitly out of scope for now (the
  fields exist — `DIVISION.DIVGUNTM`, `ANNRACE.RUNTIME/RUNCTIME` — but are
  empty in the sample and deferred).
- Writing back to RD (we already do that separately).
- Replacing the map-based resolve *mechanism* (we keep `chip_lookup` +
  `reload_chip_lookup`). Note: division-name display and "show the chip id when
  unknown" both require **small additive changes to the resolve payload**
  (`ResolvedParticipant`, `load_chip_to_participant`, the announcer/UI event) —
  see §6 and §8. So this is not a zero-touch feature on the resolve path.

## 2. Background: the pipeline already exists

The receiver already has the full ingest→store→resolve pipeline; only the
*source* is new.

- **Parsers**: `services/receiver/src/participants.rs` (`.ppl`, `.bibchip`).
- **Store (source of resilience)**: SQLite tables `participants(bib PK, last,
  first, affiliation, gender)` and `bib_chips(chip_id PK, bib)`
  (`services/receiver/src/storage/schema.sql`). Replaced wholesale on import.
- **Runtime lookup**: `AppState.chip_lookup: Arc<RwLock<ChipLookup>>` where
  `ChipLookup = HashMap<String, HashMap<String,(String,String)>>`
  (`control_api.rs`). Built from the DB by `reload_chip_lookup()`, at startup
  (`runtime.rs`) and after each import.
- **Import entry points**: `control_api::import_participants[_file]` →
  `db.replace_participants`; `.bibchip` → `db.replace_bib_chips`; both →
  `reload_chip_lookup`.
- **Resolve / unknowns**: `reload_chip_lookup` builds `chip_lookup` from
  `db.load_chip_to_participant()`, which **INNER JOINs** `bib_chips` to
  `participants` (`db.rs`) — chips with no matching participant are dropped.
  `SnapshotResolver::resolve(chip_id)` returns `Option<ResolvedParticipant {
  bib: String, name: String }>`; on `None` the read is still pushed/displayed
  with the raw `chip_id` and no bib/name (`announcer_push.rs`, `ui_events.rs`).
  There is **no** `create_unknown`-style synthetic participant in the receiver
  resolve path (a `create_unknown` exists only in the unrelated `timer-core`
  crate and is not used here).

Implication: a DBF source mostly reuses the existing replace path (produce the
same `Vec<Participant>` + `(bib, chip)` pairs). The SQLite store, in-mem map, and
replace-all contract are unchanged; the only additive touches are division names
and unknown-chip display on the resolve payload (§6/§8).

## 3. Source data (verified against Santa Hamilton 2025 sample)

Event confirmed via `EVENTNM.DBF` and `DIVISION.DBF` (`DIVEVENT="Santa Hamilton
2025"`, 462 entrants in `RACE.DBF`: 5k=329, 10k=132, test=1). `ANNRACE.DBF` has
461 — it omits the single test-division entrant, which is exactly why the two
files differ by one record. The import files are DBF v0x30 (Visual FoxPro);
`EVENTNM.DBF` is v0x32, so the reader must parse the header generically rather
than assert the version byte. Text is latin1/codepage; deleted rows are flagged
with `*` in byte 0.

| File | Role | Fields we read | Key |
|---|---|---|---|
| `checkchip.dbf` | bib ↔ chip map | `CHECK1`=bib, `CHECK2`=chip (12-hex iPico) | chip (PK), bib |
| `RACE.DBF` (or `ANNRACE.DBF`) | participant table | `RUNERNO`=bib, `RUNFNAME`, `RUNLNAME`, `RUNSEX`, `RUNDIV` | `RUNERNO` |
| `DIVISION.DBF` | division names | `DIVNO`, `DIVNAME` | `DIVNO` |

Verified join (checked against `ANNRACE`'s 461 rows — the 462nd/test-division
entrant in `RACE.DBF` was not chip-checked): every `RUNERNO` has exactly one chip
in `checkchip`; no chip maps to two bibs; `checkchip.CHECK1` == `RUNERNO` (bib ==
runner number for this event). `checkchip` also carries ~388 spare (unassigned)
chips — expected; a chip whose bib has no participant simply resolves to nothing
(see §8).

### Multiple chips per bib (required)

A bib can legitimately map to **several chips** — redundant chips on one runner,
or a shared bib in a relay/team leg. The store already supports this with **no
special handling**: `bib_chips` has `chip_id` as PK with a non-unique `bib` (plus
`idx_bib_chips_bib`), so N chips → 1 bib is just N rows. (The receiver's
`participants.rs` `Participant` is `{bib,last,first,affiliation,gender}` — it has
no `chip_id` field; the chip list lives in `bib_chips`, not on the participant.)

- `checkchip.dbf` has one row per chip, so we insert one `bib_chips` row per
  chip; multiple `CHECK1` rows with the same bib all resolve to that bib. No
  aggregation/grouping step is needed.
- RD also models two chip *types* per runner in `CHMPCHIP.DBF` / `REPTANCHIP.DBF`
  (`CHIPNOWT` = write/bib tag, `CHIPNORFID` = RFID) keyed by `RUNERNO`. For this
  event each runner has one chip and all chip files agree, so `checkchip` alone
  is sufficient; treat `CHMPCHIP`/`REPTANCHIP` as an alternate/secondary source
  only if a future event splits WT vs RFID chips.
- Full relay/team modelling (`RELAY.DBF`, `RELBIBNO`, team legs) is **out of
  scope** for MVP; we only need chip→bib→participant. Shared-bib relays still
  resolve correctly through the many-chips-per-bib path.

### Source-file choice: ANNRACE vs RACE (unresolved — needs empirical check)

We have **no vendor documentation** for either file; everything here is inferred
from the one sample + filenames.

- `RACE.DBF`: 462 records, 219 fields, most recent mtime. Looks like the
  **canonical** current-race participant table.
- `ANNRACE.DBF`: 461 records, 218 fields. The `ANN` prefix *suggests* an
  announcer table (a `REPTAN*` snapshot cluster exists alongside it), but its
  purpose and refresh trigger are **unverified**. It may be an announcer-module
  working file that is only written when RD's announcer screen is active — which
  would make it unreliable as a general source.

The two are **not identical** (record and field counts differ), so this is a
real choice, not a cosmetic one. Pending evidence, **default to `RACE.DBF`** as
the canonical, always-present table. Confirm empirically before finalizing:
watch which file(s) RD actually rewrites during a live race (procmon, the same
method used to reverse-engineer `IPICO.DBF` in `ipico-direct-dbf-format.md`).
See open question #2.

Notes / gotchas:
- Skip the deleted `BIB`/`CHIP` header row in `checkchip` (delete flag + non-numeric bib).
- The participant table (`RACE.DBF`, 219 fields; or `ANNRACE.DBF`, 218) is wide;
  we read a handful of `C` columns by name and ignore the rest (incl. memo/`V`/`T`
  fields → no `.FPT` handling needed for our subset).
- Bib is an integer (`RUNERNO`), matching the existing `i64` canonical key.
- Gender: `RUNSEX` (`M`/`F`) → `M`/`F`, else `X`. (`RUNNONBIN`/`RUNSEX2` exist;
  ignore for now.)
- `affiliation` (in the `.ppl` model) has **no obvious RD source column** in our
  read set, so RD-imported participants will have empty affiliation unless we map
  a team/club field later. Call this out rather than silently blank it.
- Division: `RUNDIV` is an int code; `DIVISION.DBF` gives the display name. The
  receiver's participant/resolve path currently has **no division field at all**
  (`ResolvedParticipant` is `{bib, name}`), so surfacing division names is net-new
  work — see §6.
- These are separate files RD may rewrite at different times → cross-file skew
  is possible (a chip's bib not yet in the participant table, or vice versa).
  Handle defensively — but note the current INNER-JOIN resolver *drops* a chip
  whose bib has no participant row (§8), so "temporarily missing name" today means
  "chip resolves to nothing", not "bib shown without name".

## 4. Options

The naive framing ("live read per lookup" vs "periodic into a DB") partly
dissolves given §2: resolution *already* runs against an in-mem map backed by
SQLite. The real axes are **(a) what triggers a refresh** and **(b) how robust
the refresh is against a live RD writing the files**. Performance is a
non-issue: all three files together are < 1 MB and parse in single-digit ms.

### A. Live parse on every chip read *(rejected)*
Open + parse the DBFs on each read resolution.
- ✅ Always fresh; no cache.
- ❌ Regresses the existing map-based resolver; reads arrive in bursts
  (many/sec across mats) → constant file I/O and constant lock contention with
  RD writing the same files. The cost is *file contention*, not CPU.
- ❌ A torn/locked read now affects live resolution instead of a background job.
- Confidence this is wrong for us: **high.**

### B. Periodic poll → replace store *(recommended baseline)*
A background task every N seconds: (optionally) detect change, snapshot-copy,
parse all three files, and — only on full success — replace the SQLite tables
and rebuild `chip_lookup` via the existing path.
- ✅ Drop-in on the existing architecture (new source, same replace path).
- ✅ Bounded, predictable contention with RD (once per interval).
- ✅ Freshness lag = interval; 5–30 s is fine for registration/check-in edits.
- ✅ Trivial cost given file sizes.
- ❌ Not instant; picks a polling interval.
- Confidence this is the right baseline: **high.**

### C. File-watch → replace store *(deferred — not in this scope)*
OS file notifications (FSEvents/`ReadDirectoryChangesW`/inotify) trigger a
debounced reload.
- ✅ More responsive, less wasted work.
- ❌ Unreliable alone: RD may write in place without close events, or via
  temp+rename; network shares drop events. Must watch `.dbf` (+ `.fpt`/`.cdx`).
- Verdict: **deferred.** Not part of the agreed D + B scope; revisit only if the
  poll interval proves too laggy in practice.

### D. Manual "Import from Race Director" action *(keep as MVP + fallback)*
A button/command that runs the same parse→replace once, on demand — the DBF
analog of today's file upload.
- ✅ Smallest first step; reuses `import_participants_file` shape exactly.
- ✅ Always-available manual override even after B ships.
- ❌ Still manual (the thing we're trying to remove) if used alone.
- Confidence it should exist regardless: **high** (ship as step 1, keep as fallback).

### Cross-cutting modifiers (apply to B/C/D)

- **Snapshot-copy then parse**: copy each DBF to a temp path, parse the copy.
  Mitigates sharing-violations and torn reads while RD writes. Cheap; recommended.
- **Change detection**: skip reparse when `(mtime, size)` (or a content hash) of
  all three files is unchanged. Avoids needless SQLite churn.
- **Keep-last-good on error**: unlike the strict all-or-nothing `.ppl` upload, a
  *background* source must never blank the board mid-race. On any parse/lock
  failure, log, keep the previous snapshot, retry next tick. Only swap on a
  fully successful parse of the whole set.
- **In-mem vs DB**: keep SQLite as the resilience layer (already there). DBF is
  the import source, not the runtime store — so the receiver can still resolve
  from the last good import if RD/DBF is briefly unavailable (RD closed, share
  unmounted) or unreadable at startup.

## 5. Decision

**Ship D + B (B configurable/toggleable).** Agreed scope:

1. **Step 1 (MVP): D** — an `import_participants_from_rd(dir)` control action
   that parses `checkchip` + the participant table (`RACE.DBF`; see §3 source
   choice) + `DIVISION` and reuses
   existing `replace_participants` + existing `replace_bib_chips` + new
   `replace_divisions` +
   `reload_chip_lookup`. Proves the parser/mapping end-to-end with zero new
   runtime machinery, and remains the manual override afterwards.
2. **Step 2: B** — a background poller wrapping step 1's logic with
   change-detection, snapshot-copy, and keep-last-good. Enabled via config with a
   configurable interval + directory; when disabled, only D (manual) runs.

Reject **A**. Defer **C**. Keep SQLite persistence. Fully reversible (source
selection is config; the store/lookup are unchanged).

## 6. Design detail (recommended path)

New module `services/receiver/src/rd_dbf.rs`:

- Minimal DBF reader (header + field descriptors + fixed-width records; honor
  the `*` delete flag; latin1 decode; select needed columns by name). Prefer a
  small hand-rolled reader over a crate since our field subset is all `C`/`N`
  and we want to skip memo files — revisit the `dbase` crate if VFP quirks bite.
- `load_from_dir(dir) -> Result<RdImport>` where `RdImport { participants:
  Vec<Participant>, chips: Vec<(i64, String)>, divisions: HashMap<i32,String> }`.
  - `checkchip.dbf` → `chips` (skip deleted/header, bib parse to `i64`, validate
    hex, **lowercase** the chip). `chip_lookup` matching is case-sensitive and
    emulator/reader frames are lowercase (`{:012x}`); the existing `.bibchip`
    parser preserves case, so lowercasing RD chips keeps both import paths
    resolving identically — confirm real reader frames are lowercase.
  - `RACE.DBF` (or `ANNRACE.DBF`, per §3) → `participants` (map fields per §3;
    `division` from `RUNDIV`).
  - `DIVISION.DBF` → `divisions` (optional; for display names).
- Reuse existing `db.replace_participants` **and** existing `db.replace_bib_chips`
  (`db.rs`, already used by the `.bibchip` path); add new `db.replace_divisions`;
  then `reload_chip_lookup`. All within the existing "replace-all" contract.

Control/runtime:
- `control_api::import_participants_from_rd(state, dir)` (mirrors
  `import_participants_file`), wired through `control_bridge`.
- Background poller task (step 2) spawned from `runtime.rs`, gated by config,
  using change-detection + copy-then-parse + keep-last-good.

Division names (**in scope now — touches the resolve payload**): add a
`divisions(divno INTEGER PRIMARY KEY, name TEXT NOT NULL)` table (replaced
wholesale like `participants`) loaded from `DIVISION.DBF` (`DIVNO`→`DIVNAME`).
Because today's `participants` table and `ResolvedParticipant {bib, name}` carry
no division, surfacing it requires, minimally: a division column on `participants`
(from `RUNDIV`), a `division`/`division_name` field on `ResolvedParticipant`, a
join change in `load_chip_to_participant` (or a second lookup against the division
map), and the extra field in the announcer/UI event. This is additive, but it is
**not** "no change to the resolve path" — scope it explicitly. (An
`Arc<RwLock<HashMap<i32,String>>>` division map in `AppState` is an alternative
to joining in SQL.)

## 7. Configuration

Config lives **where the rest of the receiver config lives**: the `profile` row
in the receiver SQLite DB, alongside the existing `dbf_enabled` / `dbf_path`
columns used by the `IPICO.DBF` *writer* (`db.rs` `DbfConfig` /
`load_dbf_config` / `save_dbf_config`). Add a parallel `RdImportConfig` backed by
new `profile` columns, wired through `control_bridge` exactly like `DbfConfig`:

- `rd_import_enabled: bool` (default false) — enables the step-B background poll.
- `rd_import_dir: path` — RD working directory containing the `.dbf` files.
- `rd_import_interval_secs: u32` (default e.g. 15) — poll cadence for B.

Filenames are **fixed** (`checkchip.dbf`, the participant table, `DIVISION.DBF`)
— RD's filenames are stable across installs, so no per-install filename override.
The participant table filename (`RACE.DBF` vs `ANNRACE.DBF`) is pending the §3
source-choice check but is itself stable once chosen.

The manual action (D) takes `dir` as an argument and works regardless of the
`enabled` toggle. Selecting the RD source does not preclude the existing
`.ppl`/`.bibchip` upload; the last successful import (from any source) wins.

## 8. Failure modes

| Case | Handling |
|---|---|
| Dir/file missing, RD closed | Log, keep last good, retry next tick. Not fatal. |
| File locked / sharing violation | Copy-then-parse; on failure keep last good, retry. |
| Torn / partial record | Validate record length; skip individual bad rows; if the file as a whole fails to parse, keep last good (do not blank). |
| Chip in `checkchip` but bib absent in participant table | **Today:** INNER-JOIN resolver drops it → resolves to nothing (no bib shown). To "show the bib while the name loads" needs a LEFT-JOIN / separate chip→bib map — out of MVP scope unless we take the resolve-payload change in §6. |
| Bib in participant table with no chip | Fine; no chip lookup entry. |
| Chip read matches nothing (truly unknown chip) | Resolver returns `None`; the read is displayed with its raw `chip_id` and no bib/name. This is what satisfies "show the unknown chip with its ID" — no synthetic participant is created. |
| Encoding / accented names | Decode latin1/codepage explicitly. |
| Manual import (D) racing the poll (B) | Both call `replace_* → reload_chip_lookup` under `state.storage.db.lock()`, so writes serialize; last-writer-wins is benign. Copy-then-parse of the 3 files is **not** atomic w.r.t. RD's writes, hence the cross-file-skew handling above. |
| Stale files from a prior event | Out of scope to auto-detect; operator points `dir` at the live event. Optionally cross-check `EVENTNM.DBF`/`DIVISION.DIVEVENT` and surface it in the UI. |

## 9. Testing (deterministic, per repo norms)

- Unit: DBF reader (header parse, delete flag, field selection, latin1) against
  small committed fixture DBFs derived from the sample.
- Unit: field mapping (participant table / `checkchip` / `DIVISION` → model),
  incl. header row skip, spare chips, missing cross-file rows, gender/division
  mapping.
- Unit: **multiple chips per bib** — synthetic `checkchip` fixture where one bib
  has 2+ chips; assert every chip resolves to that bib. (Real sample is strictly
  1:1, so this path is otherwise untested.)
- Integration: `dir` → import → SQLite → `chip_lookup` resolves a known chip to
  bib/name/division; unknown chip → unknown fallback.
- Integration (step 2): change-detection no-ops when unchanged; keep-last-good
  when a file is truncated/locked mid-poll.

## 10. Open questions

1. Is `bib == RUNERNO` guaranteed across RD installs, or do some events use a
   separate printed bib (`RACER.BIBNO` / `RUNDYBIB`)? Need a second (different)
   event sample to confirm before relying on `RUNERNO` as the bib.
2. `ANNRACE` vs `RACE.DBF` as the participant source: confirm `ANNRACE` is always
   produced/refreshed for the live event, or default to `RACE.DBF`.

Resolved this round: config location (profile row), division names now, fixed
filenames (stable), multiple chips per bib (supported — see §3), scope = D + B.
