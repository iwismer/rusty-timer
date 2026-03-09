# Contextual Help System — Implementation Plan

## Overview

Add a two-tier contextual help system to all configuration and admin UI surfaces in rusty-timer. Field-level `?` icons show compact popovers on hover and open detailed card-level modals on click. A global search lets operators find help by keyword. The goal: on race day, any operator can quickly understand any setting without external documentation.

## Current State Analysis

### What Exists
- **11 Cards** in `ForwarderConfig.svelte` (shared-ui), each with inline `<p class={hintClass}>` hint text under fields
- **5 Cards** in receiver-ui `+page.svelte` (Config, Receiver Mode, Available Streams + 2 non-config)
- **6 Cards** in receiver-ui `admin/+page.svelte` (Cursor Reset, Epoch Overrides, Port Overrides, Purge, Reset Profile, Factory Reset)
- **Read mode selectors** in both `forwarder-ui/src/routes/+page.svelte:985-1048` and `server-ui/src/routes/+page.svelte:841-903`
- **ConfirmDialog** provides the `<dialog>` pattern to model after
- **Card.svelte** accepts `title`, `headerBg`, `borderStatus`, `header` (Snippet), `children` (Snippet)
- **No tooltip/popover component exists** in the codebase

### Key Discoveries
- Card.svelte uses Svelte 5 `$props()` with inline TS types (`Card.svelte:5-18`)
- ConfirmDialog pattern: `$state` for dialogEl binding, `$effect` for open/close with cleanup, `shouldCancelOnBackdropClick`/`shouldCancelOnEscape` extracted to pure TS (`confirm-dialog.ts:1-10`)
- shared-ui tests only test pure `.ts` logic, never mount `.svelte` components (`confirm-dialog.test.ts`, `card-logic.test.ts`)
- receiver-ui tests use `@testing-library/svelte` + jsdom + `vi.mock()` for page-level component tests
- Two label patterns coexist: `text-sm font-medium text-text-secondary` (ForwarderConfig) vs `text-xs font-medium text-text-muted` (receiver-ui)
- NavBar has a right-side area (`ml-auto`) with theme toggle — help search button goes here
- ForwarderConfig has 12 `<label>` elements across 11 Cards
- Receiver admin cards have no `<label>` elements — only card-level help needed there

### Patterns to Follow
- Logic extraction: pure functions in `src/lib/*.ts` with companion `.test.ts`
- Props: `$props()` with inline TS type annotation
- Snippets: typed as `Snippet` from `"svelte"`, rendered with `{@render}`
- Callbacks: `onX: () => void` style (not event dispatchers)
- Dialog: `$effect` watching `open` prop to drive `showModal()`/`close()`

## Desired End State

- Every visible config field has an inline `?` icon that shows a popover on hover and opens a detailed modal on click
- Every Card has a `?` button in its header that opens a section-level help modal
- The help modal has anchored field headings, search filtering, race-day tips, and see-also links
- A global help search in the NavBar searches all help content across all contexts
- All help content is centralized in typed TypeScript files
- Existing inline hint text is preserved unchanged

### Verification
- `npm run check` passes in shared-ui, forwarder-ui, server-ui, receiver-ui
- `vitest run` passes in shared-ui and receiver-ui
- Visual: every `?` icon shows a popover on hover, opens the correct modal section on click
- Visual: card `?` buttons open modals with all fields listed
- Visual: NavBar search finds fields across all contexts

## What We're NOT Doing

- Removing existing `<p class={hintClass}>` hint text (kept as-is for now)
- Adding help to the server-ui dashboard pages (streams, races, announcer-config, sbc-setup) — only config/admin surfaces
- Adding a markdown parser — all help content is plain text or simple HTML spans
- i18n / externalized help content — TypeScript files are sufficient
- Help for the Log Card or Status Card (no configurable fields)

## Implementation Approach

Six phases, each independently testable:
1. **Help data layer** — types + content files + lookup functions + tests
2. **HelpDialog component** — the full modal with search, anchors, tips
3. **HelpTip component** — inline `?` icon with popover + click-to-modal
4. **Card.svelte enhancement** — `helpSection`/`helpContext` props, header `?` button
5. **Integration** — wire help into all UI surfaces
6. **Global search** — HelpSearch component in NavBar

---

## Phase 1: Help Data Layer

### Overview
Create the typed help content registry. This is the foundation — all components consume it. Content files will be written by research subagents (see Phase 1b).

### Changes Required

#### 1. Help Types
**File**: `apps/shared-ui/src/lib/help/help-types.ts` (new)

```ts
export type FieldHelp = {
  label: string;
  summary: string;
  detail: string;
  default?: string;
  range?: string;
  recommended?: string;
};

export type SectionHelp = {
  title: string;
  overview: string;
  fields: Record<string, FieldHelp>;
  tips?: string[];
  seeAlso?: { sectionKey: string; label: string }[];
};

export type HelpContext = Record<string, SectionHelp>;
export type HelpContextName = "forwarder" | "receiver" | "receiver-admin";
```

#### 2. Help Lookup Functions
**File**: `apps/shared-ui/src/lib/help/index.ts` (new)

```ts
import type { HelpContext, HelpContextName, SectionHelp, FieldHelp } from "./help-types";
import { FORWARDER_HELP } from "./forwarder-help";
import { RECEIVER_HELP } from "./receiver-help";
import { RECEIVER_ADMIN_HELP } from "./receiver-admin-help";

const CONTEXTS: Record<HelpContextName, HelpContext> = {
  forwarder: FORWARDER_HELP,
  receiver: RECEIVER_HELP,
  "receiver-admin": RECEIVER_ADMIN_HELP,
};

export function getSection(context: HelpContextName, sectionKey: string): SectionHelp | undefined {
  return CONTEXTS[context]?.[sectionKey];
}

export function getField(context: HelpContextName, sectionKey: string, fieldKey: string): FieldHelp | undefined {
  return CONTEXTS[context]?.[sectionKey]?.fields[fieldKey];
}

/** Search all help content across all contexts. Returns matches grouped by context+section. */
export function searchHelp(query: string): Array<{
  context: HelpContextName;
  sectionKey: string;
  section: SectionHelp;
  matchedFields: Array<{ fieldKey: string; field: FieldHelp }>;
  matchedTips: string[];
}> {
  if (!query.trim()) return [];
  const q = query.toLowerCase();
  const results: ReturnType<typeof searchHelp> = [];

  for (const [contextName, context] of Object.entries(CONTEXTS) as [HelpContextName, HelpContext][]) {
    for (const [sectionKey, section] of Object.entries(context)) {
      const matchedFields = Object.entries(section.fields)
        .filter(([, f]) =>
          [f.label, f.summary, f.detail, f.default, f.range, f.recommended]
            .some(text => text?.toLowerCase().includes(q))
        )
        .map(([fieldKey, field]) => ({ fieldKey, field }));

      const matchedTips = (section.tips ?? []).filter(t => t.toLowerCase().includes(q));

      const sectionMatches =
        section.title.toLowerCase().includes(q) ||
        section.overview.toLowerCase().includes(q);

      if (matchedFields.length > 0 || matchedTips.length > 0 || sectionMatches) {
        results.push({
          context: contextName,
          sectionKey,
          section,
          matchedFields: matchedFields.length > 0 ? matchedFields : Object.entries(section.fields).map(([k, f]) => ({ fieldKey: k, field: f })),
          matchedTips,
        });
      }
    }
  }
  return results;
}

export type { HelpContext, HelpContextName, SectionHelp, FieldHelp } from "./help-types";
```

#### 3. Content Files (placeholder structure — populated by subagents in Phase 1b)
**File**: `apps/shared-ui/src/lib/help/forwarder-help.ts` (new)
**File**: `apps/shared-ui/src/lib/help/receiver-help.ts` (new)
**File**: `apps/shared-ui/src/lib/help/receiver-admin-help.ts` (new)

Each follows this pattern:
```ts
import type { HelpContext } from "./help-types";

export const FORWARDER_HELP: HelpContext = {
  general: {
    title: "General Settings",
    overview: "...",
    fields: {
      display_name: {
        label: "Display Name",
        summary: "...",
        detail: "...",
        default: "None (optional)",
      },
    },
    tips: ["..."],
    seeAlso: [{ sectionKey: "server", label: "Server Connection" }],
  },
  // ... all sections
};
```

#### 4. Exports
**File**: `apps/shared-ui/src/lib/index.ts` (modify)
**Changes**: Add re-export line:
```ts
export * from "../lib/help/index";
```

#### 5. Tests
**File**: `apps/shared-ui/src/lib/help/help-lookup.test.ts` (new)

Tests for `getSection`, `getField`, `searchHelp`:
- `getSection("forwarder", "server")` returns the server section
- `getSection("forwarder", "nonexistent")` returns undefined
- `getField("forwarder", "server", "base_url")` returns the field
- `searchHelp("batch")` returns uplink section with batch fields matched
- `searchHelp("")` returns empty array
- `searchHelp("zzz-no-match")` returns empty array

Follow the shared-ui test pattern: pure logic, `{ describe, expect, it }` from vitest, no DOM.

### Phase 1b: Help Content Research (Subagent Prompts)

Three subagents run in parallel to research and write help content. Each subagent should:
1. Read the relevant source code to deeply understand each field
2. Read the spec at `docs/specs/remote-forwarding-v1.md` for protocol-level details
3. Write complete, accurate help content with race-day troubleshooting tips

**Subagent 1 prompt — Forwarder Help Content:**
```
Research and write the help content for forwarder-help.ts. You are writing help documentation
that will be shown to race timing operators on race day.

Read these files to understand every forwarder config field deeply:
- apps/shared-ui/src/components/ForwarderConfig.svelte (all 11 config sections)
- apps/shared-ui/src/lib/forwarder-config-form.ts (validation rules, defaults, constraints)
- apps/shared-ui/src/lib/read-mode-form.ts (read mode options, timeout range 1-255)
- services/forwarder/src/ — search for config struct definitions, default values, and behavior
- crates/rt-protocol/ — search for WsMessage types related to read mode
- crates/ipico-core/ — understand ReadType enum (Raw, Event, FirstLastSeen)
- docs/specs/remote-forwarding-v1.md — protocol spec for delivery semantics, journal behavior

Write apps/shared-ui/src/lib/help/forwarder-help.ts with type HelpContext (import from ./help-types).

Sections needed: general, server, readers, read_mode, controls, dangerous_actions, ws_path, auth,
journal, uplink, status_http, update.

For each field, write:
- summary: 1-2 concise sentences for the popover
- detail: Full explanation for the modal. For read_mode, explain all 3 modes (Raw, Event, FS/LS)
  in depth with pros/cons. For timeout, explain the deduplication window concept.
- default: The actual default value from the code
- range: Valid values from validation logic
- recommended: Race-day recommendation with rationale

For tips arrays, write practical race-day troubleshooting guidance:
- "If reads aren't appearing, check X first"
- "On race day, set update mode to Disabled to prevent unexpected restarts"
- "In-memory journal is fine for testing but use a file path for race day"

For read_mode specifically, recommend "First/Last Seen with a 5-second timeout" as the default
for most race timing scenarios, and explain why.

For seeAlso, link related sections (e.g., journal -> uplink, server -> ws_path).
```

**Subagent 2 prompt — Receiver Help Content:**
```
Research and write the help content for receiver-help.ts. You are writing help documentation
that will be shown to race timing operators on race day.

Read these files to understand every receiver config field deeply:
- apps/receiver-ui/src/routes/+page.svelte (Config card, Receiver Mode card, Available Streams)
- apps/receiver-ui/src/lib/api.ts (API calls, what each endpoint does)
- services/receiver/src/ — search for config structs, mode enum, stream subscription logic,
  epoch handling, cursor tracking, local port forwarding
- docs/specs/remote-forwarding-v1.md — protocol spec for stream semantics, epochs, cursors

Write apps/shared-ui/src/lib/help/receiver-help.ts with type HelpContext (import from ./help-types).

Sections needed: config, receiver_mode, streams.

For receiver_mode, write detailed explanations of all 3 modes:
- Live: auto-subscribes to all streams, supports earliest-epoch overrides
- Race: follows server-defined stream assignments, epoch controls shown but disabled
- Targeted Replay: per-stream epoch selection for historical data replay

For streams, explain:
- What a "stream" represents (forwarder + reader data feed)
- What epochs are and why they matter
- Subscribe/unsubscribe behavior
- Local port routing (how reads reach timing software)
- The difference between earliest_epoch and targeted_epoch controls

For tips, write practical guidance:
- "Use Live mode for standard race timing"
- "Switch to Targeted Replay to re-send historical data to your timing software"
- "Unsubscribing a stream only stops local delivery — data continues on the server"
```

**Subagent 3 prompt — Receiver Admin Help Content:**
```
Research and write the help content for receiver-admin-help.ts. You are writing help documentation
that will be shown to race timing operators on race day.

Read these files to understand every admin action deeply:
- apps/receiver-ui/src/routes/admin/+page.svelte (all 6 admin cards)
- apps/receiver-ui/src/lib/api.ts (API calls for each admin action)
- services/receiver/src/ — search for cursor reset, epoch override clear, port override,
  purge subscriptions, reset profile, factory reset implementations

Write apps/shared-ui/src/lib/help/receiver-admin-help.ts with type HelpContext (import from ./help-types).

Sections needed: cursor_reset, epoch_overrides, port_overrides, purge_subscriptions,
reset_profile, factory_reset.

For port_overrides, include a field entry for port_override with:
- The default port calculation formula (10000 + last IP octet)
- Valid range (1-65535)
- Common timing software port conventions

For tips on each section, write practical guidance:
- When to use each action vs alternatives
- What data is preserved vs lost
- Recovery steps after each action
- Impact on active timing (will reads be interrupted? duplicated?)

For factory_reset tips specifically:
- List alternatives to try first (cursor reset, purge subscriptions)
- Emphasize this is irreversible
- Note that the receiver must be fully reconfigured after
```

### Success Criteria
- [ ] `apps/shared-ui/src/lib/help/` directory exists with 5 files
- [ ] TypeScript compiles: `cd apps/shared-ui && npx tsc --noEmit` (or `npm run check`)
- [ ] Tests pass: `cd apps/shared-ui && npx vitest run`
- [ ] `searchHelp("batch")` returns results (manual verification in test)

---

## Phase 2: HelpDialog Component

### Overview
Build the card-level help modal. This is the Tier 2 deep-help surface — opened from card headers and from HelpTip clicks.

### Changes Required

#### 1. Dialog Logic (extracted pure functions)
**File**: `apps/shared-ui/src/lib/help-dialog.ts` (new)

```ts
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
```

#### 2. HelpDialog Component
**File**: `apps/shared-ui/src/components/HelpDialog.svelte` (new)

Props:
```ts
let {
  open = false,
  sectionKey = "",
  context = "forwarder" as HelpContextName,
  scrollToField = undefined as string | undefined,
  onClose,
}: {
  open: boolean;
  sectionKey: string;
  context: HelpContextName;
  scrollToField?: string;
  onClose: () => void;
} = $props();
```

Key behaviors:
- Uses `<dialog>` element with `showModal()`/`close()` pattern from ConfirmDialog
- `$effect` watches `open` to drive `showModal()`/`close()`
- `$effect` watches `scrollToField`: when set and dialog is open, calls `document.getElementById("help-" + scrollToField)?.scrollIntoView({ behavior: "smooth", block: "start" })`
- Search input at top filters via `filterSectionContent()`
- Each field rendered with `id="help-{fieldKey}"` for anchor scrolling
- Close on ESC, backdrop click (reuse `shouldCancelOnBackdropClick`/`shouldCancelOnEscape`)
- Styling: `max-w-2xl w-full max-h-[80vh] overflow-y-auto` on the dialog body

Template structure:
```svelte
<dialog bind:this={dialogEl} onkeydown={handleKeydown}
  class="fixed inset-0 m-auto max-w-2xl w-full rounded-lg border border-border bg-surface-1 p-0 shadow-lg backdrop:bg-black/50">
  <div class="sticky top-0 bg-surface-1 border-b border-border px-6 py-4 z-10">
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-bold text-text-primary m-0">{section.title}</h2>
      <button onclick={onClose} class="text-text-muted hover:text-text-primary text-lg cursor-pointer">✕</button>
    </div>
    <input type="text" placeholder="Search this section..." bind:value={searchQuery}
      class="mt-3 w-full px-3 py-1.5 text-sm rounded-md border border-border bg-surface-0 text-text-primary placeholder:text-text-muted focus:outline-none focus:border-accent" />
  </div>
  <div class="px-6 py-4 overflow-y-auto max-h-[70vh]">
    <p class="text-sm text-text-secondary mb-4">{section.overview}</p>

    {#each visibleFields as [fieldKey, field]}
      <div id="help-{fieldKey}" class="mb-6 scroll-mt-32">
        <h3 class="text-sm font-semibold text-text-primary mb-1">{field.label}</h3>
        <p class="text-sm text-text-secondary mb-2">{@html field.detail}</p>
        {#if field.default}
          <p class="text-xs text-text-muted">Default: <code class="bg-surface-2 px-1 rounded">{field.default}</code></p>
        {/if}
        {#if field.range}
          <p class="text-xs text-text-muted">Valid: {field.range}</p>
        {/if}
        {#if field.recommended}
          <p class="text-xs font-medium text-accent">Recommended: {field.recommended}</p>
        {/if}
      </div>
    {/each}

    {#if visibleTips.length > 0}
      <div class="mt-6 pt-4 border-t border-border">
        <h3 class="text-sm font-semibold text-text-primary mb-2">Race-Day Tips</h3>
        <ul class="list-disc list-inside space-y-1">
          {#each visibleTips as tip}
            <li class="text-sm text-text-secondary">{@html tip}</li>
          {/each}
        </ul>
      </div>
    {/if}

    {#if section.seeAlso?.length}
      <div class="mt-6 pt-4 border-t border-border">
        <h3 class="text-sm font-semibold text-text-primary mb-2">See Also</h3>
        {#each section.seeAlso as link}
          <button onclick={() => navigateToSection(link.sectionKey)}
            class="text-sm text-accent hover:underline cursor-pointer mr-4">
            {link.label}
          </button>
        {/each}
      </div>
    {/if}
  </div>
</dialog>
```

Note: "See Also" links call `onClose()` then re-open with a different `sectionKey`. This requires the parent to manage which section is open. The `navigateToSection` callback should be an optional prop: `onNavigate?: (sectionKey: string) => void`.

#### 3. Tests
**File**: `apps/shared-ui/src/lib/help-dialog.test.ts` (new)

Test `filterSectionContent`:
- Empty query returns all fields and tips
- Matching query filters to relevant fields only
- Query matching a tip returns that tip
- Case-insensitive matching
- No matches returns empty arrays

### Success Criteria
- [ ] `filterSectionContent` tests pass
- [ ] TypeScript compiles in shared-ui
- [ ] HelpDialog can be rendered standalone (verified manually or via receiver-ui test)

---

## Phase 3: HelpTip Component

### Overview
Build the field-level `?` icon with hover popover and click-to-modal behavior.

### Changes Required

#### 1. HelpTip Logic (extracted pure function)
**File**: `apps/shared-ui/src/lib/help-tip.ts` (new)

```ts
/** Determine popover position: "below" by default, "above" if near viewport bottom. */
export function resolvePopoverPosition(
  buttonRect: { bottom: number; top: number },
  viewportHeight: number,
  popoverHeight: number = 200,
): "above" | "below" {
  const spaceBelow = viewportHeight - buttonRect.bottom;
  return spaceBelow < popoverHeight && buttonRect.top > popoverHeight ? "above" : "below";
}
```

#### 2. HelpTip Component
**File**: `apps/shared-ui/src/components/HelpTip.svelte` (new)

Props:
```ts
let {
  fieldKey,
  sectionKey,
  context = "forwarder" as HelpContextName,
  onOpenModal,
}: {
  fieldKey: string;
  sectionKey: string;
  context: HelpContextName;
  onOpenModal?: (fieldKey: string) => void;
} = $props();
```

Key behaviors:
- Looks up `getField(context, sectionKey, fieldKey)` on mount to get content
- Hover: 200ms delay via `setTimeout`, shows popover, clears on mouseleave
- Click: calls `onOpenModal?.(fieldKey)` — the parent (Card or page) handles opening HelpDialog
- Focus: shows popover (keyboard accessibility)
- Enter key: calls `onOpenModal?.(fieldKey)`
- Popover positioned via `resolvePopoverPosition()` — uses a `$state` ref on the button and measures on show

Template:
```svelte
<span class="relative inline-flex items-center ml-1">
  <button
    bind:this={btnEl}
    onmouseenter={scheduleShow}
    onmouseleave={scheduleHide}
    onfocus={showPopover}
    onblur={scheduleHide}
    onclick={handleClick}
    onkeydown={handleKeydown}
    class="inline-flex items-center justify-center w-4 h-4 rounded-full border border-border text-text-muted hover:text-accent hover:border-accent focus:text-accent focus:border-accent text-[10px] font-bold cursor-pointer bg-transparent transition-colors"
    aria-label="Help for {field?.label ?? fieldKey}"
    type="button"
  >?</button>

  {#if showingPopover && field}
    <div
      class="absolute z-50 w-72 p-3 rounded-lg border border-border bg-surface-1 shadow-lg text-sm {positionClass}"
      role="tooltip"
      onmouseenter={cancelHide}
      onmouseleave={scheduleHide}
    >
      <p class="text-text-primary mb-1">{field.summary}</p>
      {#if field.default}
        <p class="text-xs text-text-muted">Default: <code class="bg-surface-2 px-1 rounded">{field.default}</code></p>
      {/if}
      {#if field.range}
        <p class="text-xs text-text-muted">Valid: {field.range}</p>
      {/if}
      {#if field.recommended}
        <p class="text-xs font-medium text-accent">Recommended: {field.recommended}</p>
      {/if}
      {#if onOpenModal}
        <button onclick={handleClick} class="mt-2 text-xs text-accent hover:underline cursor-pointer">
          More details...
        </button>
      {/if}
    </div>
  {/if}
</span>
```

Position classes:
```ts
let positionClass = $derived(
  popoverPosition === "above"
    ? "bottom-full mb-2 left-0"
    : "top-full mt-2 left-0"
);
```

#### 3. Tests
**File**: `apps/shared-ui/src/lib/help-tip.test.ts` (new)

Test `resolvePopoverPosition`:
- Plenty of space below → returns "below"
- Near bottom of viewport → returns "above"
- Near top and bottom → returns "below" (default)

### Success Criteria
- [ ] `resolvePopoverPosition` tests pass
- [ ] TypeScript compiles in shared-ui
- [ ] HelpTip renders inline and shows popover (verified manually)

---

## Phase 4: Card.svelte Enhancement

### Overview
Add `helpSection` and `helpContext` optional props to Card. When set, render a `?` button in the header that opens HelpDialog.

### Changes Required

#### 1. Modify Card.svelte
**File**: `apps/shared-ui/src/components/Card.svelte` (modify)

Add imports:
```ts
import type { HelpContextName } from "../lib/help/help-types";
import HelpDialog from "./HelpDialog.svelte";
```

Add props (extend existing destructuring at lines 5-18):
```ts
let {
  title = undefined,
  headerBg = false,
  borderStatus = undefined,
  helpSection = undefined,
  helpContext = undefined,
  header,
  children,
}: {
  title?: string;
  headerBg?: boolean;
  borderStatus?: "ok" | "warn" | "err";
  helpSection?: string;
  helpContext?: HelpContextName;
  header?: Snippet;
  children?: Snippet;
} = $props();
```

Add state:
```ts
let helpDialogOpen = $state(false);
let helpScrollToField = $state<string | undefined>(undefined);

function openHelp(fieldKey?: string) {
  helpScrollToField = fieldKey;
  helpDialogOpen = true;
}
function closeHelp() {
  helpDialogOpen = false;
  helpScrollToField = undefined;
}
```

Modify header template (line 31-41). The key change is adding a `?` button when `helpSection` is set:

**Old** (lines 31-41):
```svelte
{#if title || header}
  <div class="px-4 py-3 border-b border-border flex flex-wrap items-center gap-3 {headerBgClass}">
    {#if header}
      {@render header()}
    {:else}
      <h2 class="text-sm font-semibold text-text-primary">{title}</h2>
    {/if}
  </div>
{/if}
```

**New**:
```svelte
{#if title || header || helpSection}
  <div class="px-4 py-3 border-b border-border flex flex-wrap items-center gap-3 {headerBgClass}">
    {#if header}
      {@render header()}
    {:else}
      <h2 class="text-sm font-semibold text-text-primary">{title}</h2>
    {/if}
    {#if helpSection && helpContext}
      <button
        onclick={() => openHelp()}
        class="ml-auto inline-flex items-center justify-center w-5 h-5 rounded-full border border-border text-text-muted hover:text-accent hover:border-accent text-xs font-bold cursor-pointer bg-transparent transition-colors"
        aria-label="Help for {title ?? helpSection}"
        type="button"
      >?</button>
    {/if}
  </div>
{/if}
```

Add HelpDialog at the end of the component template:
```svelte
{#if helpSection && helpContext}
  <HelpDialog
    open={helpDialogOpen}
    sectionKey={helpSection}
    context={helpContext}
    scrollToField={helpScrollToField}
    onClose={closeHelp}
  />
{/if}
```

Expose `openHelp` so child HelpTip components can call it. Two options:
- **Option A**: Pass `openHelp` down via a context/prop. Since HelpTips are inside Card's `children` snippet, they need a way to reach `openHelp`. The cleanest approach: Card sets a Svelte context that HelpTip reads.

Add to Card.svelte script:
```ts
import { setContext } from "svelte";
setContext("help-open-modal", openHelp);
```

HelpTip.svelte reads it:
```ts
import { getContext } from "svelte";
const openModal = getContext<((fieldKey?: string) => void) | undefined>("help-open-modal");
```

This way, HelpTip doesn't need an explicit `onOpenModal` prop when used inside a Card with `helpSection` — it discovers the callback via context. The `onOpenModal` prop remains as an override for cases where HelpTip is used outside a Card.

#### 2. Logic Test (Card)
**File**: `apps/shared-ui/src/lib/card-logic.test.ts` (modify)

No changes needed — `resolveHeaderBgClass` is unchanged. The Card `?` button is purely template logic.

#### 3. Exports
**File**: `apps/shared-ui/src/lib/index.ts` (modify)

Add exports for HelpTip and HelpDialog:
```ts
export { default as HelpTip } from "../components/HelpTip.svelte";
export { default as HelpDialog } from "../components/HelpDialog.svelte";
```

### Success Criteria
- [ ] TypeScript compiles in shared-ui
- [ ] Existing Card tests still pass
- [ ] `<Card title="Test" helpSection="server" helpContext="forwarder">` renders `?` button in header
- [ ] `<Card title="Test">` (no helpSection) renders identically to before

---

## Phase 5: Integration

### Overview
Wire HelpTip and helpSection into all UI surfaces. This is the largest phase by line count but is mechanical — adding props and components to existing templates.

### Changes Required

#### 1. ForwarderConfig.svelte
**File**: `apps/shared-ui/src/components/ForwarderConfig.svelte` (modify)

Add import:
```ts
import HelpTip from "./HelpTip.svelte";
```

**Add `helpSection` + `helpContext` to every Card** (11 Cards):

| Card (line) | `helpSection` | `helpContext` |
|---|---|---|
| General (449) | `"general"` | `"forwarder"` |
| Server (474) | `"server"` | `"forwarder"` |
| Readers (502) | `"readers"` | `"forwarder"` |
| Forwarder Controls (653) | `"controls"` | `"forwarder"` |
| Dangerous Actions (689) | `"dangerous_actions"` | `"forwarder"` |
| Forwarders WS Path (740) | `"ws_path"` | `"forwarder"` |
| Auth (765) | `"auth"` | `"forwarder"` |
| Journal (792) | `"journal"` | `"forwarder"` |
| Uplink (824) | `"uplink"` | `"forwarder"` |
| Status HTTP (865) | `"status_http"` | `"forwarder"` |
| Update (890) | `"update"` | `"forwarder"` |

Example change for Server Card (line 474):
```svelte
<!-- Before -->
<Card title="Server">

<!-- After -->
<Card title="Server" helpSection="server" helpContext="forwarder">
```

**Add `<HelpTip>` to every label** (12 labels):

Example change for Base URL label (line 476-479):
```svelte
<!-- Before -->
<label class="block text-sm font-medium text-text-secondary">
  Base URL
  <input type="text" bind:value={serverBaseUrl} class="mt-1 {inputClass}" />

<!-- After -->
<label class="block text-sm font-medium text-text-secondary">
  Base URL <HelpTip fieldKey="base_url" sectionKey="server" context="forwarder" />
  <input type="text" bind:value={serverBaseUrl} class="mt-1 {inputClass}" />
```

Full label-to-HelpTip mapping:

| Label text | `fieldKey` | `sectionKey` |
|---|---|---|
| Display Name (450) | `"display_name"` | `"general"` |
| Base URL (476) | `"base_url"` | `"server"` |
| Allow restart/shutdown (658) | `"allow_power_actions"` | `"controls"` |
| WebSocket Path (741) | `"forwarders_ws_path"` | `"ws_path"` |
| Token File Path (766) | `"token_file"` | `"auth"` |
| SQLite Path (794) | `"sqlite_path"` | `"journal"` |
| Prune Watermark % (799) | `"prune_watermark_pct"` | `"journal"` |
| Batch Mode (826) | `"batch_mode"` | `"uplink"` |
| Batch Flush (ms) (835) | `"batch_flush_ms"` | `"uplink"` |
| Batch Max Events (840) | `"batch_max_events"` | `"uplink"` |
| Bind Address (866) | `"bind"` | `"status_http"` |
| Update Mode (891) | `"update_mode"` | `"update"` |

**Readers table columns** — Add HelpTip to column headers:

In the `<thead>` (line 509-524), add HelpTip after column header text:
```svelte
<th class="text-left py-2 px-2 text-xs font-medium text-text-muted" colspan="2">
  IP Address <HelpTip fieldKey="reader_ip" sectionKey="readers" context="forwarder" />
</th>
<th class="text-left py-2 px-2 text-xs font-medium text-text-muted w-24">
  Reader Port <HelpTip fieldKey="reader_port" sectionKey="readers" context="forwarder" />
</th>
<!-- etc. for Enabled, Default Local Port, Local Port Override -->
```

#### 2. Forwarder-UI Read Mode
**File**: `apps/forwarder-ui/src/routes/+page.svelte` (modify)

Add import:
```ts
import { HelpTip } from "@rusty-timer/shared-ui";
```

At line 985, modify the read mode label:
```svelte
<!-- Before -->
<span class="text-text-muted">Read Mode:</span>

<!-- After -->
<span class="text-text-muted">Read Mode: <HelpTip fieldKey="read_mode" sectionKey="read_mode" context="forwarder" /></span>
```

At the timeout label (around line 1012):
```svelte
<!-- Before -->
<span>Timeout</span>

<!-- After -->
<span>Timeout <HelpTip fieldKey="timeout" sectionKey="read_mode" context="forwarder" /></span>
```

Note: Since these HelpTips are not inside a Card with helpSection, the context-based `openHelp` won't be available. The HelpTip will show the popover on hover but the "More details..." link will be a no-op (or we can render a standalone HelpDialog here). The simplest approach: add a local HelpDialog instance for the `read_mode` section and wire `onOpenModal` on the HelpTip to open it.

#### 3. Server-UI Read Mode
**File**: `apps/server-ui/src/routes/+page.svelte` (modify)

Same pattern as forwarder-ui. At line 843:
```svelte
<span class="text-sm text-text-muted">Read Mode: <HelpTip fieldKey="read_mode" sectionKey="read_mode" context="forwarder" /></span>
```

And for the timeout label (around line 869).

#### 4. Receiver-UI Main Page
**File**: `apps/receiver-ui/src/routes/+page.svelte` (modify)

Add import:
```ts
import { HelpTip } from "@rusty-timer/shared-ui";
```

Add `helpSection`/`helpContext` to Cards:
- Config Card (921): `helpSection="config" helpContext="receiver"`
- Receiver Mode Card (1054): `helpSection="receiver_mode" helpContext="receiver"`
- Available Streams Card (1125): This uses `{#snippet header()}`. Add `helpSection="streams" helpContext="receiver"` to the Card tag. The Card component will render its `?` button alongside the custom header snippet.

Add HelpTip to Config Card labels (lines 983-1026):
```svelte
<label class="block text-xs font-medium text-text-muted">
  Receiver ID <HelpTip fieldKey="receiver_id" sectionKey="config" context="receiver" />
  <input ... />
</label>
```

Repeat for Server URL (`server_url`), Token (`token`), Update Mode (`update_mode`).

Add HelpTip to Receiver Mode label (line 1057):
```svelte
<label class="block text-xs font-medium text-text-muted">
  Mode <HelpTip fieldKey="mode" sectionKey="receiver_mode" context="receiver" />
  <select ...>
```

#### 5. Receiver-UI Admin Page
**File**: `apps/receiver-ui/src/routes/admin/+page.svelte` (modify)

Add `helpSection`/`helpContext` to all 6 Cards:

| Card (line) | `helpSection` | `helpContext` |
|---|---|---|
| Cursor Reset (232) | `"cursor_reset"` | `"receiver-admin"` |
| Earliest-Epoch Overrides (316) | `"epoch_overrides"` | `"receiver-admin"` |
| Local Port Overrides (373) | `"port_overrides"` | `"receiver-admin"` |
| Purge Subscriptions (430) | `"purge_subscriptions"` | `"receiver-admin"` |
| Reset Profile (453) | `"reset_profile"` | `"receiver-admin"` |
| Factory Reset (475) | `"factory_reset"` | `"receiver-admin"` |

For the Port Overrides table header "Port Override" at line 387, add a HelpTip:
```svelte
<th ...>Port Override <HelpTip fieldKey="port_override" sectionKey="port_overrides" context="receiver-admin" /></th>
```

### Success Criteria
- [ ] `npm run check` passes in all 4 app packages (shared-ui, forwarder-ui, server-ui, receiver-ui)
- [ ] `npx vitest run` passes in shared-ui and receiver-ui
- [ ] Visual: every Card header shows a `?` button
- [ ] Visual: every field label has an inline `?` icon
- [ ] Visual: hovering a `?` shows the popover; clicking opens the modal scrolled to that field

---

## Phase 6: Global Help Search

### Overview
Add a HelpSearch component to the NavBar for searching all help content across all contexts.

### Changes Required

#### 1. HelpSearch Component
**File**: `apps/shared-ui/src/components/HelpSearch.svelte` (new)

Props:
```ts
let {
  context = undefined as HelpContextName | undefined,
}: {
  context?: HelpContextName;
} = $props();
```

When `context` is set, only searches that context. When unset, searches all.

Behaviors:
- Renders a search icon button in the NavBar area
- Clicking it opens a search input (either inline expanding or a dropdown)
- As the user types, calls `searchHelp(query)` and shows results in a dropdown
- Results grouped by section, showing field label + summary
- Clicking a result opens a HelpDialog for that section+field
- ESC or click-outside closes the search

Template sketch:
```svelte
<div class="relative">
  <button onclick={toggleSearch} class="..." aria-label="Search help" type="button">
    <!-- magnifying glass SVG icon -->
  </button>
  {#if searchOpen}
    <div class="absolute right-0 top-full mt-2 w-96 bg-surface-1 border border-border rounded-lg shadow-lg z-50 overflow-hidden">
      <input type="text" bind:value={query} placeholder="Search help..."
        class="w-full px-4 py-2 text-sm border-b border-border bg-surface-0 ..." />
      <div class="max-h-80 overflow-y-auto">
        {#each results as result}
          <div class="px-4 py-2 border-b border-border">
            <h4 class="text-xs font-semibold text-text-muted">{result.section.title}</h4>
            {#each result.matchedFields as { fieldKey, field }}
              <button onclick={() => openResult(result, fieldKey)}
                class="block w-full text-left py-1 hover:bg-surface-2 rounded px-2 cursor-pointer">
                <span class="text-sm text-text-primary">{field.label}</span>
                <span class="text-xs text-text-muted ml-2">{field.summary}</span>
              </button>
            {/each}
          </div>
        {/each}
        {#if query && results.length === 0}
          <p class="px-4 py-3 text-sm text-text-muted">No results found.</p>
        {/if}
      </div>
    </div>
  {/if}
</div>

<!-- HelpDialog opened from search results -->
<HelpDialog open={dialogOpen} sectionKey={dialogSection} context={dialogContext}
  scrollToField={dialogField} onClose={() => { dialogOpen = false; }} />
```

#### 2. NavBar Enhancement
**File**: `apps/shared-ui/src/components/NavBar.svelte` (modify)

Add a `helpContext` optional prop and render HelpSearch in the right-side area (before the theme toggle):
```ts
import HelpSearch from "./HelpSearch.svelte";

// In props:
helpContext = undefined as HelpContextName | undefined,
```

In the template (right-side div, before theme toggle):
```svelte
{#if helpContext}
  <HelpSearch context={helpContext} />
{/if}
```

#### 3. Wire NavBar helpContext in each app

Each app's layout or page passes `helpContext` to NavBar:
- `forwarder-ui`: `helpContext="forwarder"`
- `server-ui`: `helpContext="forwarder"` (it configures forwarders)
- `receiver-ui`: `helpContext="receiver"` (search includes receiver-admin too — or pass `undefined` to search all)

#### 4. Exports
**File**: `apps/shared-ui/src/lib/index.ts` (modify)

Add:
```ts
export { default as HelpSearch } from "../components/HelpSearch.svelte";
```

### Success Criteria
- [ ] TypeScript compiles in all apps
- [ ] NavBar shows a search icon when `helpContext` is set
- [ ] Typing "batch" in search shows Uplink section results
- [ ] Clicking a result opens HelpDialog scrolled to that field
- [ ] ESC closes the search dropdown

---

## Testing Strategy

### Unit Tests (shared-ui, pure logic):
- `help-lookup.test.ts`: `getSection`, `getField`, `searchHelp` — core data lookup and search
- `help-dialog.test.ts`: `filterSectionContent` — section-level search filtering
- `help-tip.test.ts`: `resolvePopoverPosition` — popover flip logic

### Integration Tests (receiver-ui, component-level):
- Extend `admin-page.test.ts`: verify `?` buttons are present in Card headers (`screen.getByRole("button", { name: /Help for/ })`)
- Extend `+page.svelte.test.ts`: verify HelpTip renders next to known fields

### Manual Testing:
1. Open forwarder-ui config page — verify every Card has header `?` and every field has inline `?`
2. Hover a field `?` — verify popover appears with correct content
3. Click a field `?` — verify modal opens scrolled to that field
4. Click a Card header `?` — verify modal opens at top
5. Use search in NavBar — verify results appear and clicking one opens the modal
6. Test keyboard: Tab to `?`, Enter opens modal, ESC closes
7. Test dark mode: verify all help components use theme tokens correctly

## Performance Considerations

- Help content is statically imported — no lazy loading needed for this volume (~30 fields)
- `searchHelp()` iterates all content on every keystroke. With ~30 fields across 3 contexts this is negligible. If content grows significantly, debounce the search input (200ms)
- HelpDialog uses `{#if}` conditional rendering — it's not in the DOM when closed
- Popover show/hide uses 200ms `setTimeout` — prevents flicker on fast mouse movements

## References

- Design spec: `.context/contextual-help-spec.md`
- ConfirmDialog pattern: `apps/shared-ui/src/components/ConfirmDialog.svelte`
- Card component: `apps/shared-ui/src/components/Card.svelte`
- ForwarderConfig integration target: `apps/shared-ui/src/components/ForwarderConfig.svelte`
- Test patterns: `apps/shared-ui/src/lib/confirm-dialog.test.ts`, `apps/receiver-ui/src/test/+page.svelte.test.ts`
- Frozen v1 spec: `docs/specs/remote-forwarding-v1.md`

## Execution

Use `rpi-implement` to execute this plan phase by phase. Each phase should be committed separately. Phase 1b (help content research) should use parallel subagents with the prompts specified above — these can run concurrently with Phase 2 and 3 component work since the content files just need to match the `HelpContext` type interface.
