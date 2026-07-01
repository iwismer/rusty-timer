# Race Director DBF file catalog

Reference for the `.DBF` files Race Director (RD) keeps in an event working
directory. Compiled from a Santa Hamilton 2025 instance (`~/Nextcloud/scratch/dbf`,
~240 files). RD is a Visual FoxPro app; files are DBF v0x30, latin1/codepage
text, fixed-width records, deleted rows flagged with `*` in byte 0. Memo/`V`/`T`
fields imply sidecar `.FPT`/`.CDX` files we don't need for our subset.

Purpose descriptions are **inferred from schema + contents**; treat the "core"
and "chip/participant" rows as verified and the rest as best-effort.

## How to re-inspect

The quick header/field dump used to build this (adjust the path):

```python
import struct, glob, os
def hdr(p):
    with open(p,'rb') as f:
        h=f.read(32); num=struct.unpack('<I',h[4:8])[0]
        hlen=struct.unpack('<H',h[8:10])[0]; rlen=struct.unpack('<H',h[10:12])[0]
        fields=[]
        while True:
            fd=f.read(32)
            if not fd or fd[0]==0x0D or len(fd)<32: break
            fields.append(fd[0:11].split(b'\x00')[0].decode('latin1'))
        return num, fields
```

Delete flag: read from `hlen`, each record is `rlen` bytes, byte 0 is `*`
(deleted) or space (live). Decode text as latin1.

## Key finding: mixed staleness

RD only rewrites files relevant to the **currently loaded** event, so mtimes are
mixed and unreliable — trust *content*, not the file date. In this snapshot the
live Santa Hamilton 2025 files (`ANNRACE`, `checkchip`, `DIVISION`, `RACE`,
`EVENTNM`, `RSLTNAMES`, `SPLASHMSG`, …) coexist with stale leftovers from other
events (`CHIPRACE`, `RACER`, `PARTRACE`, `REPTTR1`, …). Confirm an event via
`EVENTNM.DBF` / `DIVISION.DIVEVENT`.

## Naming conventions (prefixes)

- `RUN*` fields — per-participant "runner" columns (name, div, chip, times, places).
- `REPTOV*` / `OV*` — "overall" report snapshots (multi-event roll-ups).
- `REPTAN*` — "announce" report snapshots (paired with the announcer board).
- `REPTAG*` / `AG*` — "age-graded" / aggregate report snapshots.
- `REPTDC*` / `RD*` / `RL*` / `RFID*` — reader/decoder export sets (per chip system).
- `TEMP*` / `*temp*` — transient working copies from an operation.
- `*BHD` / `*FH` — "banner header" / "footer header" report metadata rows.

## Core current-race files (verified — our integration surface)

| File | Recs | Purpose |
|---|---|---|
| `EVENTNM.DBF` | 1 | Event identity. `EVENTNM`="Santa Hamilton 2025". |
| `DIVISION.DBF` | 3 | Divisions for the loaded event: `DIVNO`→`DIVNAME` (1=5k, 2=10k, 3=test), `DIVEVENT`, counts, and (empty here) gun/wave-time fields (`DIVGUNTM`, `DIVMXSTRT`, `DIVWTIME`). |
| `RACE.DBF` | 462 | Current-race participant table (219-col `RUN*` schema); most complete/recent — likely the canonical source. |
| `ANNRACE.DBF` | 461 | Near-identical participant table (218 cols). `ANN` prefix *suggests* an announcer table, but its purpose/refresh trigger is **unverified** (no vendor docs). Differs slightly from `RACE.DBF` in record + field count. |
| `checkchip.dbf` | 850 (849 live + 1 deleted header) | **bib ↔ chip map**. `CHECK1`=bib, `CHECK2`=chip (12-hex iPico). Row 1 is a deleted `BIB`/`CHIP` header; the 849 live rows include spare unassigned chips. Our default chip source. |

## Chip-assignment files (several representations of the same data)

| File | Recs | Key → chip | Notes |
|---|---|---|---|
| `checkchip.dbf` | 849 live | bib → chip | Flat, one row per chip (850 total incl. 1 deleted header row). Cleanest for chip→bib. |
| `CHMPCHIP.DBF` | 849 | `RUNERNO` → `CHIPNOWT` + `CHIPNORFID` | Two chip *types* per runner (write tag + RFID). |
| `REPTANCHIP.DBF` | 849 | `RUNERNO` → chips | Announcer report chip snapshot (paired w/ ANNRACE). |
| `REPTAGCHIP.DBF` | 1501 | `RUNERNO` → chips | Age-graded report chip snapshot (stale/other event). |
| `REPTOVCHIP.DBF` | 898 | `RUNERNO` → chips | Overall report chip snapshot (stale). |
| `RFIDTRAN.DBF` | 849 | `RUNERNO` → `CHIPNORFID` | RFID transponder export. |
| `RLCHIP.DBF` | 849 | `RUNERNO` → `CHIPNO` | Reader/decoder chip export. |
| `trichip.dbf` | 0 | `RUNERNO` → `CHIPNORFID` | Tri-chip export (empty). |
| `CHIPRACE.DBF` | 153 | chip → `RUNERNO`+name+div | **Stale** (May 2025, different event). |
| `CHIPLIST1.DBF` | 67 | bib list w/ times | Chip-list report (stale). |
| `CHIPBROW.DBF` / `CHIPOVER.DBF` / `CHIPOVR.DBF` | small/0 | chip browse/override scratch. |

## Participant / registry files

| File | Recs | Purpose |
|---|---|---|
| `PARTFILE.DBF` | 171,722 | **All-time master person DB** (`PARTID`, name, address, email, `PCHIP`). Not per-race, no bib. |
| `PARTDTL.DBF` | 395,536 | `PARTID` ↔ race/year history links. |
| `PARTRACE.DBF` | 23 | Per-race participant record (stale 2024). |
| `PARTEXP.DBF` | 372 | Participant export snapshot (stale). |
| `importfl.dbf` | 581 | Last import working file (`RUN*` schema). |
| `activefl.dbf` | 9 | Active-filter working set. |
| `reg_in.dbf` / `event_in.dbf` / `quest_in.dbf` | small | Registration import staging (RunSignup/etc.). |
| `RSUPARTS.DBF` | 3322 | RunSignup participant sync cache. |
| `HISTORY.DBF` | 0 | Participant race history (`RUN*` 125-col). |
| `seriesa.dbf` / `SERPART.DBF` / `SERIES*` | var | Race-series standings/participants. |

## Results / timing / raw reads

| File | Recs | Purpose |
|---|---|---|
| `RAWREADH.DBF` | 792,154 | **Raw reads history** — `CHIPNO`, `BIBNO`, `SYSDATE`/`SYSTIME`, `TPOINT`, `READER`, `USED`/`OVERRIDE`. |
| `RAWREADS.DBF` | 0 | Raw reads working table (adds `NETTIME`/`WAVETIME`/`NAME`). |
| `IPICO.DBF` | 42 | The iPico read-import file (what **we write** — see `ipico-direct-dbf-format.md`). `EVENT,DIVISION,CHIP,TIME,RUNERNO,DAYCODE,LAPNO,TPOINT,READER`. |
| `HOLDRSLT.DBF` | 1694 | Held/queued results (`EVENT,CHIP,TIME,RUNERNO,DAYCODE`) — stale 2013. |
| `HOLDSTRT.DBF` / `HOLDCHMP.DBF` | 0 | Held start / chip-champ scratch. |
| `RFID.DBF` / `RRESULT.DBF` / `racerslt.dbf` / `readlite.dbf` / `chronotr.dbf` | 0 | Per-chip-system result import tables (empty). |
| `rslttime.dbf` / `multread.dbf` / `splitck.dbf` / `NOMATCH.DBF` | var | Result timing / multi-read / split / unmatched-read scratch. |
| `ASRESULT.DBF` | 173 | Age-standard results (stale). |
| `LAPTIMES.DBF` / `LAPTIMEH.DBF` / `laps*` / `*LAPTM*` | var | Lap timing tables. |
| `AGTIME.DBF` / `OVTIME.DBF` / `TIME(S).DBF` | var | Position/time capture tables. |

## Announcer / leaderboard / display

| File | Recs | Purpose |
|---|---|---|
| `ANNRACE.DBF` | 461 | Announcer participant table (see core). |
| `announce.dbf` | 0 | Announcer live-scroll row buffer (`BIBNO,NAME,CITY,AGE,TIME,NETTIME,DIVNO,DIVISION,…`). |
| `ANNTP.DBF` / `TPOINTS*.DBF` | 1 | Timing-point definitions (`TPNO,TPDESC,TPOCCUR,…`). |
| `ALERTS.DBF` | 0 | Announcer alert rows (VIP/callout). |
| `SPLASHMSG.dbf` | 461 | Per-bib splash/announcer message. |
| `RSLTNAMES.DBF` | 461 | `RUNERNO,TEAMSEQ,BIBNO,NAME` display-name lookup for the live event. |
| `LBCATS.dbf` / `LBDIVS.dbf` | 48/3 | Leaderboard category / division selections. |

## Divisions / age groups / factors / teams

| File | Recs | Purpose |
|---|---|---|
| `AGEGROUP.DBF` / `AGEBANDS.DBF` | var | Age-group / age-band definitions + records. |
| `ASFACTORS.DBF` (+ `ASFACT*`) | 5300 | Age-standard/age-grading factor tables. |
| `DIVEVENT.DBF` / `divlist.dbf` / `DIVMAP*.DBF` | var | Division-to-event/segment mapping. |
| `TEAM.DBF` / `DIVTEAM.DBF` / `TEAMCL.DBF` / `teamts.dbf` | var | Team scoring definitions. |
| `RELAY.DBF` / `RELAYH.DBF` / `*RELAY*` | 0 | Relay team legs (`RELTEAMNO,RELBIBNO,RELFNAME,…`). |
| `XCDIVS.DBF` / `XCSCORE.dbf` / `NCAATB.DBF` / `SEGTF.DBF` | var | Cross-country / track scoring. |

## Report snapshots (regenerated per report run — usually ignore)

Families cloned from the participant/division/chip tables at report time:
`REPTOV*` (overall), `REPTAN*` (announce), `REPTAG*` (age-graded), `REPTKS*`,
`REPTTR1`/`REPTTM2`, `REPTDC*`, plus `OVRACE`/`OVDIV`/`OVTIME`, `AGRACE`,
`RACEATHL`, `REPTFILE`, `LABELS`, `custlab`/`custrept`. `RACEFH`/`Racebhd`/`*BHD`/
`*FH` hold report banner/footer + weather/header metadata (`BHDDATE`,
`BHDWEATHER`, participant counts, etc.).

## Config / reference / lookup data

| File | Recs | Purpose |
|---|---|---|
| `MISC.DBF` | 432 | Misc key/value config (`MISCTYPE,MISCID,MISCEXT`); `MISCSAVE.DBF` is a 722k-row archive. |
| `EVENT.DBF` / `EventTemp.DBF` | 0/2 | Per-division event/distance config (`EVENT1..5`, `EVDIST*`). |
| `country.dbf` / `valstate*.dbf` / `zipcode.dbf` / `timezone.dbf` | large | Geographic reference lookups. |
| `FNAME.DBF` / `SEARCHNAMES.DBF` | var | Name gender/search helpers. |
| `COLMAP.DBF` / `racemap*.DBF` / `raceimp.DBF` | var | Column/field import mapping. |
| `IPICOCC1..8.DBF` / `IPICOTEST.DBF` | 0/3205 | iPico reader per-channel output + test capture. |
| `Volser.dbf`, `AUTOSAVE.DBF`, `TKDEBUG.DBF`, `SSSAVE.DBF` | tiny | Volume serial / autosave / debug scratch. |

## Files most relevant to us

- **Read for participant import**: `checkchip.dbf` (chip↔bib), `RACE.DBF`
  (bib→name/division; `ANNRACE.DBF` is an unverified alternative — see spec §3),
  `DIVISION.DBF` (division names).
  See `participant-dbf-import.md`.
- **We already write**: `IPICO.DBF`. See `ipico-direct-dbf-format.md`.
- **Potential future (deferred)**: gun/wave time in `DIVISION.DBF`
  (`DIVGUNTM`/`DIVMXSTRT`), finish/net times in `ANNRACE`/`RACE`
  (`RUNTIME`/`RUNCTIME`), raw reads in `RAWREADH.DBF`.
