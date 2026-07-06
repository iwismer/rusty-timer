import { describe, expect, it } from "vitest";
import {
  getField,
  getSection,
  type HelpContextName,
} from "@rusty-timer/shared-ui";
import readerControlPanel from "../../../../shared-ui/src/components/ReaderControlPanel.svelte?raw";
import adminTab from "./AdminTab.svelte?raw";
import cursorResetSection from "./admin/CursorResetSection.svelte?raw";
import dangerActionsSection from "./admin/DangerActionsSection.svelte?raw";
import earliestEpochSection from "./admin/EarliestEpochSection.svelte?raw";
import portOverridesSection from "./admin/PortOverridesSection.svelte?raw";
import announcerTab from "./AnnouncerTab.svelte?raw";
import configTab from "./ConfigTab.svelte?raw";
import connectionsTab from "./ConnectionsTab.svelte?raw";
import receiverModeConfig from "./ReceiverModeConfig.svelte?raw";
import statusBar from "./StatusBar.svelte?raw";
import streamsTab from "./StreamsTab.svelte?raw";

type SourceUnit = {
  name: string;
  source: string;
  variables?: Record<string, string>;
};

type FieldLookup = {
  context: HelpContextName;
  section: string;
  field: string;
  source: string;
};

type SectionLookup = {
  context: HelpContextName;
  section: string;
  source: string;
};

const components: Record<string, SourceUnit[]> = {
  AdminTab: [
    { name: "AdminTab.svelte", source: adminTab },
    { name: "CursorResetSection.svelte", source: cursorResetSection },
    { name: "EarliestEpochSection.svelte", source: earliestEpochSection },
    { name: "PortOverridesSection.svelte", source: portOverridesSection },
    { name: "DangerActionsSection.svelte", source: dangerActionsSection },
  ],
  AnnouncerTab: [{ name: "AnnouncerTab.svelte", source: announcerTab }],
  ConfigTab: [
    { name: "ConfigTab.svelte", source: configTab },
    { name: "ReceiverModeConfig.svelte", source: receiverModeConfig },
  ],
  ConnectionsTab: [
    { name: "ConnectionsTab.svelte", source: connectionsTab },
    {
      name: "ReaderControlPanel.svelte",
      source: readerControlPanel,
      variables: { helpContext: "forwarder" },
    },
  ],
  StatusBar: [{ name: "StatusBar.svelte", source: statusBar }],
  StreamsTab: [{ name: "StreamsTab.svelte", source: streamsTab }],
};

function readComponent(name: string): string {
  return components[name].map(({ source }) => source).join("\n");
}

function attrValue(tag: string, attr: string): string | undefined {
  const match = tag.match(
    new RegExp(`\\b${attr}=("[^"]*"|'[^']*'|\\{[^}]+\\})`),
  );
  return match?.[1];
}

function resolveAttr(
  rawValue: string | undefined,
  attr: string,
  sourceName: string,
  variables: Record<string, string> = {},
): string | undefined {
  if (rawValue === undefined) return undefined;
  if (rawValue.startsWith('"') || rawValue.startsWith("'")) {
    return rawValue.slice(1, -1);
  }

  const expression = rawValue.slice(1, -1).trim();
  const resolved = variables[expression];
  if (resolved !== undefined) return resolved;
  throw new Error(
    `unsupported dynamic ${attr}={${expression}} in ${sourceName}`,
  );
}

function resolveHelpContext(
  value: string | undefined,
  sourceName: string,
  variables?: Record<string, string>,
): HelpContextName | undefined {
  const resolved = resolveAttr(value, "context", sourceName, variables);
  if (!resolved) return undefined;
  if (
    !["forwarder", "receiver", "receiver-admin", "server"].includes(resolved)
  ) {
    throw new Error(`unsupported help context ${resolved} in ${sourceName}`);
  }
  return resolved as HelpContextName;
}

function extractHelpTips(sources: SourceUnit[]): FieldLookup[] {
  return sources.flatMap(({ name, source, variables }) =>
    [...source.matchAll(/<HelpTip\b[^>]*>/g)]
      .map((match) => ({ tag: match[0], index: match.index ?? 0 }))
      .map(({ tag, index }) => {
        const field = resolveAttr(
          attrValue(tag, "fieldKey"),
          "fieldKey",
          name,
          variables,
        );
        const section = resolveAttr(
          attrValue(tag, "sectionKey"),
          "sectionKey",
          name,
          variables,
        );
        const context = resolveHelpContext(
          attrValue(tag, "context"),
          name,
          variables,
        );
        if (!field || !section || !context) return undefined;
        return { context, section, field, source: `${name}:${index}` };
      })
      .filter((lookup): lookup is FieldLookup => lookup !== undefined),
  );
}

function extractHelpCards(sources: SourceUnit[]): SectionLookup[] {
  return sources.flatMap(({ name, source, variables }) =>
    [...source.matchAll(/<Card\b[^>]*>/g)]
      .map((match) => ({ tag: match[0], index: match.index ?? 0 }))
      .map(({ tag, index }) => {
        const section = resolveAttr(
          attrValue(tag, "helpSection"),
          "helpSection",
          name,
          variables,
        );
        const context = resolveHelpContext(
          attrValue(tag, "helpContext"),
          name,
          variables,
        );
        if (!section && !context) return undefined;
        if (!section || !context) {
          throw new Error(`incomplete Card help wiring in ${name}:${index}`);
        }
        return { context, section, source: `${name}:${index}` };
      })
      .filter((lookup): lookup is SectionLookup => lookup !== undefined),
  );
}

const helpTipLookups = Object.values(components).flatMap(extractHelpTips);
const cardLookups = Object.values(components).flatMap(extractHelpCards);

describe("receiver UI help coverage", () => {
  it.each(helpTipLookups)(
    "resolves $context/$section/$field from $source",
    ({ context, section, field }) => {
      expect(getField(context, section, field)).toBeDefined();
    },
  );

  it.each(cardLookups)(
    "resolves Card section $context/$section from $source",
    ({ context, section }) => {
      expect(getSection(context, section)).toBeDefined();
    },
  );

  it("wires config help through nested ReceiverModeConfig", () => {
    const configLookups = extractHelpTips(components.ConfigTab);
    expect(configLookups).toContainEqual(
      expect.objectContaining({
        context: "receiver",
        section: "receiver_mode",
        field: "mode",
      }),
    );
  });

  it("wires reset stream data to receiver-admin cursor reset help", () => {
    expect(helpTipLookups).toContainEqual(
      expect.objectContaining({
        context: "receiver-admin",
        section: "cursor_reset",
        field: "reset_stream_data",
      }),
    );
  });

  it("wires shared reader controls rendered by ConnectionsTab", () => {
    expect(helpTipLookups).toContainEqual(
      expect.objectContaining({
        context: "forwarder",
        section: "reader_live",
        field: "advance_epoch",
      }),
    );
  });

  it("does not render duplicate help icons in the Stream column header", () => {
    const source = readComponent("StreamsTab");
    const streamIdentityIndex = source.indexOf('fieldKey="stream_identity"');
    const lastReadIndex = source.indexOf('fieldKey="last_read"');
    const streamHeader = source.slice(streamIdentityIndex, lastReadIndex);

    expect(streamIdentityIndex).toBeGreaterThanOrEqual(0);
    expect(lastReadIndex).toBeGreaterThan(streamIdentityIndex);
    expect(streamHeader.match(/<HelpTip/g) ?? []).toHaveLength(1);
    expect(streamHeader).not.toContain('fieldKey="status_indicator"');
  });

  it("enables each tab's help modal", () => {
    for (const component of [
      "ConfigTab",
      "ConnectionsTab",
      "StreamsTab",
      "AnnouncerTab",
      "StatusBar",
    ]) {
      expect(readComponent(component)).toContain("onOpenModal={openHelp}");
    }
    expect(readComponent("AdminTab")).toContain("onOpenModal={openHelp}");
  });
});
