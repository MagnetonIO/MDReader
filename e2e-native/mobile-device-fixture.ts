import { randomUUID } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { checkPngFixture } from './png-fixture.ts';

/** Public input bytes only. No account, folder grant, provider or private profile. */
export function mobileDeviceFixture() {
  const run = randomUUID();
  const marker = `cedar${run.replaceAll('-', '')}`;
  const name = `mobile-reader-${run}.md`;
  const companion = `mobile-companion-${run}.md`;
  const lines = [
    '# Offline reading handbook', '',
    `The ${marker} passage belongs to this exact imported copy.`, '',
    '![Selected diagram](figures/diagram.png)', '',
    '![Selected JPEG](figures/selected.jpg)', '',
    '![Unselected diagram](figures/unselected.png)', '',
    `[Companion note](${companion})`, '',
    '## Reading checkpoint', '',
    'Pins and appearance should survive closing and reopening the app.', '',
  ];
  const checkpoint = 'Checkpoint 040';
  let checkpointLine = 0;
  // Public prose creates real overflow and enough blocks for virtualization.
  // Reading place is selected through the product outline, never native writes.
  for (let section = 1; section <= 96; section += 1) {
    const heading = `Checkpoint ${String(section).padStart(3, '0')}`;
    if (heading === checkpoint) checkpointLine = lines.length + 1;
    lines.push(`## ${heading}`, '',
      `Section ${section} explains how a reader follows a source without changing its original bytes.`, '',
      'A longer paragraph gives the phone a genuine reading column to reflow when its font or text size changes. The saved place belongs to this exact imported revision.', '',
      'The next checkpoint continues the same public handbook. Every section has a distinct heading for ordinary outline navigation.', '');
  }
  return {
    name, companion, marker, checkpoint, checkpointLine,
    markdown: '\uFEFF' + lines.join('\r\n'),
    companionMarkdown: '# Companion note\n\nA separate import keeps a separate source identity.\n',
    images: [
      { name: 'diagram.png', target: 'figures/diagram.png', alt: 'Selected diagram', mime: 'image/png',
        bytes: checkPngFixture(readFileSync(new URL('../fixtures/corpus/assets/diagram.png', import.meta.url))) },
      { name: 'selected.jpg', target: 'figures/selected.jpg', alt: 'Selected JPEG', mime: 'image/jpeg',
        bytes: readFileSync(new URL('./fixtures/selected.jpg', import.meta.url)) },
    ],
  };
}
