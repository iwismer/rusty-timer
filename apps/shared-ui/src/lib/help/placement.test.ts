import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const readerControlPanel = readFileSync(
  new URL('../../components/ReaderControlPanel.svelte', import.meta.url),
  'utf8'
);

describe('reader help icon placement', () => {
  it('does not put field help between status labels and values', () => {
    const labels = [
      'Reads (session):',
      'Reads (total):',
      '{localPortLabel}:',
      'Last seen:',
      'Current epoch:',
      'Created:',
      'Clock Drift:',
      'Read Mode:',
      'TTO Bytes:',
    ];

    for (const label of labels) {
      const index = readerControlPanel.indexOf(label);
      expect(index, `${label} should exist`).toBeGreaterThanOrEqual(0);
      const following = readerControlPanel.slice(index + label.length, index + label.length + 80);
      expect(following, `${label} should not be followed immediately by HelpTip`).not.toMatch(
        /^\s*\{#if onOpenHelpModal\}\s*<HelpTip/
      );
    }
  });

  it('keeps button action help after the button it describes', () => {
    for (const label of ['Sync Clock', 'Refresh', 'Download Reads', 'Clear Records']) {
      const index = readerControlPanel.indexOf(`>${label}</button`);
      expect(index, `${label} button should exist`).toBeGreaterThanOrEqual(0);
      const following = readerControlPanel.slice(index, index + 220);
      expect(following, `${label} button should be followed by its HelpTip`).toContain('<HelpTip');
    }
  });
});
