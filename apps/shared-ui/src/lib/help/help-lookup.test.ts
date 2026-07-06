import { describe, expect, it } from 'vitest';
import forwarderPageSource from '../../../../forwarder-ui/src/routes/+page.svelte?raw';
import forwarderConfigSource from '../../components/ForwarderConfig.svelte?raw';
import readerControlPanelSource from '../../components/ReaderControlPanel.svelte?raw';
import { getSection, getField, searchHelp } from './index';
import { FORWARDER_HELP } from './forwarder-help';
import { RECEIVER_HELP } from './receiver-help';
import { RECEIVER_ADMIN_HELP } from './receiver-admin-help';
import { SERVER_HELP } from './server-help';
import type { HelpContextName, HelpContext } from './help-types';

const ALL_CONTEXTS: Record<HelpContextName, HelpContext> = {
  forwarder: FORWARDER_HELP,
  receiver: RECEIVER_HELP,
  'receiver-admin': RECEIVER_ADMIN_HELP,
  server: SERVER_HELP,
};

const htmlTagPattern = /<\/?[a-z][a-z0-9-]*(?:\s|>|\/>)/i;

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

const SHARED_HELP_SOURCES: SourceUnit[] = [
  { name: 'ForwarderConfig.svelte', source: forwarderConfigSource },
  { name: 'ReaderControlPanel.svelte', source: readerControlPanelSource, variables: { helpContext: 'forwarder' } },
  { name: 'forwarder-ui/routes/+page.svelte', source: forwarderPageSource },
];

function expectPlainText(value: string | undefined, label: string) {
  if (value === undefined) return;
  expect(value, `${label} should be plain text, not HTML`).not.toMatch(htmlTagPattern);
}

function attrValue(tag: string, attr: string): string | undefined {
  const match = tag.match(new RegExp(`\\b${attr}=("[^"]*"|'[^']*'|\\{[^}]+\\})`));
  if (!match) return undefined;
  return match[1];
}

function resolveAttr(
  rawValue: string | undefined,
  attr: string,
  sourceName: string,
  variables: Record<string, string> = {}
): string | undefined {
  if (rawValue === undefined) return undefined;
  if (rawValue.startsWith('"') || rawValue.startsWith("'")) {
    return rawValue.slice(1, -1);
  }

  const expression = rawValue.slice(1, -1).trim();
  const resolved = variables[expression];
  if (resolved !== undefined) return resolved;
  throw new Error(`unsupported dynamic ${attr}={${expression}} in ${sourceName}`);
}

function resolveHelpContext(value: string | undefined, sourceName: string, variables?: Record<string, string>) {
  const resolved = resolveAttr(value, 'context', sourceName, variables);
  if (!resolved) return undefined;
  if (!Object.hasOwn(ALL_CONTEXTS, resolved)) {
    throw new Error(`unsupported help context ${resolved} in ${sourceName}`);
  }
  return resolved as HelpContextName;
}

function extractHelpTips(sources: SourceUnit[]): FieldLookup[] {
  return sources.flatMap(({ name, source, variables }) =>
    [...source.matchAll(/<HelpTip\b[^>]*>/g)]
      .map((match) => ({ tag: match[0], index: match.index ?? 0 }))
      .map(({ tag, index }) => {
        const field = resolveAttr(attrValue(tag, 'fieldKey'), 'fieldKey', name, variables);
        const section = resolveAttr(attrValue(tag, 'sectionKey'), 'sectionKey', name, variables);
        const context = resolveHelpContext(attrValue(tag, 'context'), name, variables);
        if (!field || !section || !context) return undefined;
        return { context, section, field, source: `${name}:${index}` };
      })
      .filter((lookup): lookup is FieldLookup => lookup !== undefined)
  );
}

function extractHelpCards(sources: SourceUnit[]): SectionLookup[] {
  return sources.flatMap(({ name, source, variables }) =>
    [...source.matchAll(/<Card\b[^>]*>/g)]
      .map((match) => ({ tag: match[0], index: match.index ?? 0 }))
      .map(({ tag, index }) => {
        const section = resolveAttr(attrValue(tag, 'helpSection'), 'helpSection', name, variables);
        const context = resolveHelpContext(attrValue(tag, 'helpContext'), name, variables);
        if (!section && !context) return undefined;
        if (!section || !context) {
          throw new Error(`incomplete Card help wiring in ${name}:${index}`);
        }
        return { context, section, source: `${name}:${index}` };
      })
      .filter((lookup): lookup is SectionLookup => lookup !== undefined)
  );
}

describe('getSection', () => {
  it('returns the p2p section for forwarder context', () => {
    const section = getSection('forwarder', 'p2p');
    expect(section).toBeDefined();
    expect(section!.title).toBe('P2P / Server');
  });

  it('returns undefined for a nonexistent section', () => {
    expect(getSection('forwarder', 'nonexistent')).toBeUndefined();
  });
});

describe('getField', () => {
  it('returns the server_url field from forwarder p2p section', () => {
    const field = getField('forwarder', 'p2p', 'server_url');
    expect(field).toBeDefined();
    expect(field!.label).toBe('Server URL');
  });

  it('returns undefined for a nonexistent field', () => {
    expect(getField('forwarder', 'p2p', 'nonexistent')).toBeUndefined();
  });

  it('returns undefined for a nonexistent section', () => {
    expect(getField('forwarder', 'nonexistent', 'server_url')).toBeUndefined();
  });
});

describe('searchHelp', () => {
  it('returns empty array for empty query', () => {
    expect(searchHelp('')).toEqual([]);
  });

  it('returns empty array for whitespace-only query', () => {
    expect(searchHelp('   ')).toEqual([]);
  });

  it('returns empty array when nothing matches', () => {
    expect(searchHelp('zzz-no-match-xyz')).toEqual([]);
  });

  it('finds forwarder p2p section when searching for server content', () => {
    const results = searchHelp('Server URL');
    expect(results.length).toBeGreaterThan(0);
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'p2p');
    expect(match).toBeDefined();
    expect(match!.matchedFields.some((f) => f.fieldKey === 'server_url')).toBe(true);
  });

  it('matches section title', () => {
    const results = searchHelp('P2P / Server');
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'p2p');
    expect(match).toBeDefined();
  });

  it('matches case-insensitively', () => {
    const results = searchHelp('SERVER URL');
    expect(results.length).toBeGreaterThan(0);
  });

  it('matches tips', () => {
    const results = searchHelp('descriptive name');
    expect(results.length).toBeGreaterThan(0);
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'general');
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });

  it('returns all fields when only section title matches', () => {
    const results = searchHelp('P2P / Server');
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'p2p');
    expect(match).toBeDefined();
    const sectionFieldCount = Object.keys(FORWARDER_HELP.p2p.fields).length;
    expect(match!.matchedFields).toHaveLength(sectionFieldCount);
    expect(match!.matchedFields.some((f) => f.fieldKey === 'server_url')).toBe(true);
  });

  it('matches section overview text', () => {
    const results = searchHelp('IPICO');
    expect(results.length).toBeGreaterThan(0);
    const match = results.find((r) => r.context === 'forwarder' && r.sectionKey === 'readers');
    expect(match).toBeDefined();
  });

  it('matches receiver-admin purge subscription tips', () => {
    const results = searchHelp('broader cleanup');
    const match = results.find(
      (r) => r.context === 'receiver-admin' && r.sectionKey === 'purge_subscriptions'
    );
    expect(match).toBeDefined();
    expect(match!.matchedTips.length).toBeGreaterThan(0);
  });
});

describe('template wiring validation', () => {
  it.each(extractHelpTips(SHARED_HELP_SOURCES))(
    'resolves $context/$section/$field from $source',
    ({ context, section, field }) => {
      expect(getField(context, section, field)).toBeDefined();
    }
  );

  it.each(extractHelpCards(SHARED_HELP_SOURCES))(
    'resolves section $context/$section from $source',
    ({ context, section }) => {
      expect(getSection(context, section)).toBeDefined();
    }
  );
});

describe('help content validation', () => {
  it('keeps plain-text help fields free of HTML markup', () => {
    for (const [contextName, context] of Object.entries(ALL_CONTEXTS)) {
      for (const [sectionKey, section] of Object.entries(context)) {
        expectPlainText(section.title, `${contextName}.${sectionKey}.title`);
        expectPlainText(section.overview, `${contextName}.${sectionKey}.overview`);
        for (const [fieldKey, field] of Object.entries(section.fields)) {
          expectPlainText(field.label, `${contextName}.${sectionKey}.${fieldKey}.label`);
          expectPlainText(field.summary, `${contextName}.${sectionKey}.${fieldKey}.summary`);
          expectPlainText(field.default, `${contextName}.${sectionKey}.${fieldKey}.default`);
          expectPlainText(field.range, `${contextName}.${sectionKey}.${fieldKey}.range`);
          expectPlainText(
            field.recommended,
            `${contextName}.${sectionKey}.${fieldKey}.recommended`
          );
        }
      }
    }
  });

  it('rejects multi-letter HTML tags in plain-text fields', () => {
    expect(() => expectPlainText('<strong>not plain</strong>', 'example.summary')).toThrow();
  });

  it('all seeAlso references resolve to existing sections', () => {
    const errors: string[] = [];
    for (const [contextName, context] of Object.entries(ALL_CONTEXTS)) {
      for (const [sectionKey, section] of Object.entries(context)) {
        for (const link of section.seeAlso ?? []) {
          if (!context[link.sectionKey]) {
            errors.push(
              `${contextName}/${sectionKey} -> seeAlso "${link.sectionKey}" does not exist`
            );
          }
        }
      }
    }
    expect(errors).toEqual([]);
  });
});
