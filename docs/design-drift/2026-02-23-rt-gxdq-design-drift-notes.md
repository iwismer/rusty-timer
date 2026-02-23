# rt-gxdq v1.1 Design Drift Notes (2026-02-23)

## Scope

This note captures shipped behavior for receiver selection/replay and forwarder current-epoch naming controls.

## Shipped behavior

- Receiver default selection mode is `manual`.
- Receiver default replay behavior is `resume`.
- `race/current` selection exists as explicit opt-in behavior.
- Receiver UI supports targeted replay with explicit per-row save behavior.
- Forwarder UI supports setting and clearing current epoch names via the server-backed path.

## Notable deltas from earlier expectations

- `race/current` is not the default behavior; operators must opt in.
- Manual selection with resume replay remains the baseline operator path.
- Targeted replay persistence is explicit per row rather than implicit bulk-save behavior.

## Operator impact

- Default operations remain predictable for existing manual workflows.
- Race-day teams can opt into `race/current` and epoch naming without changing baseline defaults.
- Per-row save in targeted replay reduces accidental persistence during selective recovery/replay operations.
