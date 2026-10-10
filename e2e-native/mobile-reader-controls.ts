import { $, browser, expect } from '@wdio/globals';
import { readerOutlineSelector, readerPanelSelector } from './mobile-reader-selectors.ts';
import type { ReaderPanel } from './mobile-reader-selectors.ts';

const sheet = '[role="dialog"][aria-modal="true"]';

/** All changes use the actual compact reader controls, not native mutations. */
export async function readerPanel(panel: ReaderPanel) {
  const trigger = await $('button[aria-label="Reader options"]');
  if (await trigger.getAttribute('aria-expanded') !== 'true') await trigger.click();
  await expect(await $(sheet)).toBeDisplayed();
  await (await $(readerPanelSelector(panel))).click();
  await expect(await $(readerPanelSelector(panel))).toHaveAttribute('aria-selected', 'true');
}

export async function closeReaderPanel() {
  await (await $(`${sheet} button[aria-label="Close reader options"]`)).click();
  await expect(await $(sheet)).not.toBeDisplayed();
}

export async function readerOutline(heading: string) {
  await readerPanel('Outline');
  const entry = await $(readerOutlineSelector(heading));
  await entry.scrollIntoView();
  await entry.click();
  await expect(await $(sheet)).not.toBeDisplayed();
}

export async function readingSize() {
  return (await $(`${sheet} output[aria-label="Text size"]`)).getText();
}

export async function changeReadingSize(direction: 'Smaller' | 'Larger') {
  const before = await readingSize();
  await (await $(`${sheet} button[aria-label="${direction} text"]`)).click();
  await browser.waitUntil(async () => await readingSize() !== before,
    { timeout: 5_000, timeoutMsg: 'Reader text size did not change through its control' });
}

export async function baselineReadingAppearance() {
  await readerPanel('Appearance');
  await (await $('//section[@role="dialog"]//label[normalize-space(.)="Serif"]')).click();
  // noReset preserves the owned E2E profile. Establish a bounded baseline by UI.
  for (let step = 0; step < 6 && await readingSize() !== '100%'; step += 1) {
    const percent = Number.parseInt(await readingSize(), 10);
    if (!Number.isFinite(percent)) throw new Error('Invalid reader text size label');
    await changeReadingSize(percent < 100 ? 'Larger' : 'Smaller');
  }
  await expect(await $(`${sheet} output[aria-label="Text size"]`)).toHaveText('100%');
  await closeReaderPanel();
}

export interface ReadingObservation {
  block: number;
  line: number;
  offset: number;
  font: string;
  fontSize: number;
  scrollTop: number;
  viewport: number;
}

/** Read-only geometry from the real source block currently crossing the top
 * edge. No scrolling, store writes, reader-handle calls or IPC mutations. */
export async function readingObservation(): Promise<ReadingObservation | null> {
  return browser.execute(() => {
    const reader = document.querySelector<HTMLElement>('.mobile-reader__document .mdreader');
    if (!reader) return null;
    const top = reader.getBoundingClientRect().top;
    const visible = Array.from(reader.querySelectorAll<HTMLElement>('[data-block-index]'))
      .map(node => ({ node, rect: node.getBoundingClientRect() }))
      .filter(({ rect }) => rect.top <= top + 1 && rect.bottom > top)
      .sort((a, b) => b.rect.top - a.rect.top)[0];
    const source = visible?.node.querySelector<HTMLElement>('[data-line-start]');
    if (!visible || !source) return null;
    const style = getComputedStyle(reader);
    return {
      block: Number(visible.node.dataset.blockIndex), line: Number(source.dataset.lineStart),
      offset: Math.max(0, top - visible.rect.top), font: style.fontFamily,
      fontSize: Number.parseFloat(style.fontSize), scrollTop: reader.scrollTop,
      viewport: reader.clientHeight,
    };
  });
}

export async function stableReadingObservation(
  matches: (value: ReadingObservation) => boolean,
): Promise<ReadingObservation> {
  let result: ReadingObservation | null = null;
  let previous: ReadingObservation | null = null;
  await browser.waitUntil(async () => {
    const current = await readingObservation();
    const stable = current && previous && current.block === previous.block
      && current.line === previous.line && Math.abs(current.offset - previous.offset) < 1
      && current.font === previous.font && current.fontSize === previous.fontSize;
    previous = current;
    if (!current || !stable || !matches(current)) return false;
    result = current;
    return true;
  }, { timeout: 15_000, interval: 150, timeoutMsg: 'Reader source anchor or computed appearance did not settle' });
  if (!result) throw new Error('No settled reader observation');
  return result;
}

export function sameReadingPlace(before: ReadingObservation, after: ReadingObservation) {
  return before.block === after.block && before.line === after.line
    && Math.abs(before.offset - after.offset) <= 4;
}
