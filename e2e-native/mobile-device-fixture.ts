import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';

/** Public input bytes only. No account, folder grant, provider or private profile. */
export function mobileDeviceFixture() {
  const run = randomUUID();
  const marker = `cedar${run.replaceAll('-', '')}`;
  const name = `mobile-reader-${run}.md`;
  const companion = `mobile-companion-${run}.md`;
  return {
    name, companion, marker,
    markdown: '\uFEFF' + [
      '# Offline reading handbook', '',
      `The ${marker} passage belongs to this exact imported copy.`, '',
      '![Selected diagram](figures/diagram.png)', '',
      '![Unselected diagram](figures/unselected.png)', '',
      `[Companion note](${companion})`, '',
      '## Reading checkpoint', '',
      'Pins and appearance should survive closing and reopening the app.', '',
    ].join('\r\n'),
    companionMarkdown: '# Companion note\n\nA separate import keeps a separate source identity.\n',
    image: readFileSync(new URL('../fixtures/corpus/assets/diagram.png', import.meta.url)),
    imageTarget: 'figures/diagram.png',
  };
}
