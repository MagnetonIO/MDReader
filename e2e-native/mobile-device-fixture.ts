import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { checkPngFixture } from './png-fixture.ts';

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
      '![Selected JPEG](figures/selected.jpg)', '',
      '![Unselected diagram](figures/unselected.png)', '',
      `[Companion note](${companion})`, '',
      '## Reading checkpoint', '',
      'Pins and appearance should survive closing and reopening the app.', '',
    ].join('\r\n'),
    companionMarkdown: '# Companion note\n\nA separate import keeps a separate source identity.\n',
    images: [
      { name: 'diagram.png', target: 'figures/diagram.png', alt: 'Selected diagram', mime: 'image/png',
        bytes: checkPngFixture(readFileSync(new URL('../fixtures/corpus/assets/diagram.png', import.meta.url))) },
      { name: 'selected.jpg', target: 'figures/selected.jpg', alt: 'Selected JPEG', mime: 'image/jpeg',
        bytes: readFileSync(new URL('./fixtures/selected.jpg', import.meta.url)) },
    ],
  };
}
