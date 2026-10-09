/** Current mobile acceptance over the real Appium WEBVIEW and native SQLite
 * owner. Public selected files need no account or remote source setup.
 * This proves local durability, not radio isolation, OS pickers or cloud sync. */
import { $, browser, expect } from '@wdio/globals';
import type { DeviceDocument } from '../../ui/src/bindings.ts';
import { E2E_BUNDLE_IDENTIFIER } from '../flows.ts';
import { MOBILE_NAVIGATION_TEXT_PATTERN } from '../mobile-assertions.ts';
import { mobileDeviceFixture } from '../mobile-device-fixture.ts';
import { chooseFiles, nativeRead, nativeReply, switchToMobileWebview } from '../mobile-device-driver.ts';

const fixture = mobileDeviceFixture();
const documentButton = `button[aria-label="${fixture.name}"]`;
let imported: DeviceDocument;

async function exactOriginal() {
  const read = await nativeRead('mobile_reader_read', { id: imported.id, revision: imported.revision });
  await expect(read.markdown).toBe(fixture.markdown);
  await expect(read.document.source_id).toBe(imported.source_id);
  await expect(read.document.revision).toBe(imported.revision);
}

async function selectedImage() {
  const reply = await nativeRead('mobile_reader_assets', {
    request: { operation: 'manifest', document_id: imported.id, revision: imported.revision },
  });
  if (reply.operation !== 'manifest') throw new Error('Expected exact asset manifest');
  await expect(reply.value.document_id).toBe(imported.id);
  await expect(reply.value.document_revision).toBe(imported.revision);
  await expect(reply.value.pending).toHaveLength(0);
  await expect(reply.value.assets).toHaveLength(1);
  const asset = reply.value.assets[0]!;
  await expect(asset.target).toBe(fixture.imageTarget);
  await expect(asset.mime).toBe('image/png');
  await expect(asset.bytes).toBe(fixture.image.length);
  const opened = await nativeRead('mobile_reader_assets', { request: {
    operation: 'open', document_id: imported.id, revision: imported.revision,
    asset_id: asset.asset_id, manifest: reply.value.manifest,
  } });
  if (opened.operation !== 'opened') throw new Error('Expected selected image session');
  try {
    const chunk = await nativeRead('mobile_reader_assets', {
      request: { operation: 'chunk', session_id: opened.session_id, offset: 0 },
    });
    if (chunk.operation !== 'chunk') throw new Error('Expected selected image bytes');
    await expect(chunk.complete).toBe(true);
    await expect(Buffer.from(chunk.base64, 'base64').equals(fixture.image)).toBe(true);
  } finally {
    await nativeRead('mobile_reader_assets', { request: { operation: 'close', session_id: opened.session_id } });
  }
}

async function renderedImage() {
  // Selected assets load on intersection; make the actual Markdown image visible.
  await (await $('.mobile-offline-image')).scrollIntoView();
  await expect(await $('img[alt="Selected diagram"]')).toExist();
  await browser.waitUntil(async () => browser.execute(() => {
    const selected = document.querySelector<HTMLImageElement>('img[alt="Selected diagram"]');
    return Boolean(selected?.complete && selected.naturalWidth > 0 && selected.src.startsWith('blob:'));
  }), { timeout: 15_000, timeoutMsg: 'Selected offline PNG was not decoded' });
  await expect(await $('img[alt="Unselected diagram"]')).not.toExist();
}

describe('MDReader native mobile device library', () => {
  before(async () => {
    const capabilities = browser.capabilities as Record<string, unknown>;
    const identity = capabilities['appium:bundleId'] ?? capabilities['appium:appPackage'];
    if (identity !== E2E_BUNDLE_IDENTIFIER) throw new Error('Refusing non-E2E application identity');
    await browser.setTimeout({ script: 30_000 });
    await switchToMobileWebview();
  });

  it('shows current source boundaries and imports selected public Markdown', async () => {
    await expect(await $('nav[aria-label="Primary"]')).toHaveText(MOBILE_NAVIGATION_TEXT_PATTERN);
    await expect(await $('.mobile-shell--device')).toExist();
    await expect(await $('button=Split')).not.toExist();
    await (await $('button=Sources')).click();
    await expect(await $('h2=Imported Markdown')).toExist();
    await expect(await $('h2=Google Drive')).toExist();
    await expect(await $('h2=Dropbox')).toExist();
    await expect(await $('h2=Enterprise CatDB server')).not.toExist();
    await expect(await $('button=Add folder')).not.toExist();
    await expect(await $('p*=Unavailable until mobile secure sign-in')).toExist();
    await (await $('button=Library')).click();
    await chooseFiles('input[aria-label="Import Markdown"]', [
      { name: fixture.name, base64: Buffer.from(fixture.markdown).toString('base64'), mime: 'text/markdown' },
      { name: fixture.companion, base64: Buffer.from(fixture.companionMarkdown).toString('base64'), mime: 'text/markdown' },
    ]);
    await expect(await $(documentButton)).toExist();
    await expect(await $(`button[aria-label="${fixture.companion}"]`)).toExist();
    const library = await nativeRead('mobile_reader_list');
    const document = library.documents.find((value) => value.name === fixture.name);
    const companion = library.documents.find((value) => value.name === fixture.companion);
    if (!document || !companion) throw new Error('Both selected imports must be published');
    imported = document;
    await expect(imported.id).not.toBe(companion.id);
    await expect(imported.source_id).not.toBe(companion.source_id);
    await exactOriginal();
  });

  it('searches a passage and opens the exact revision without semantic-model consent', async () => {
    await (await $('button=Search')).click();
    await (await $('input[type="search"]')).setValue(fixture.marker);
    await expect(await $(documentButton)).toExist();
    await expect(await $('button=Download & enable')).toExist();
    await expect(await $('select')).toHaveValue('text');
    await (await $(documentButton)).click();
    await expect(await $('main[aria-label="Reader"] h1')).toHaveText('Offline reading handbook');
    await (await $('button=Evidence')).click();
    await expect(await $('main[aria-label="Document evidence"]')).toExist();
    await expect(await $(`dd=${imported.revision}`)).toExist();
    await expect(await $(`dd=${imported.source_id}`)).toExist();
    const stale = await nativeReply('mobile_reader_read', { id: imported.id, revision: '0'.repeat(64) });
    await expect(stale.ok).toBe(false);
    if (!stale.ok) await expect(stale.error).toMatch(/revision changed/i);
    await exactOriginal();
    await (await $('button=Evidence')).click();
  });

  it('pins the document and selects only the chosen offline PNG', async () => {
    await (await $('button=Pin')).click();
    await expect(await $('button=Unpin')).toExist();
    await (await $('summary=Offline images')).click();
    await (await $('select[aria-label="Image target in this Markdown"]')).selectByAttribute('value', fixture.imageTarget);
    await chooseFiles('input[aria-label="Choose the individual image"]', [{
      name: 'diagram.png', base64: fixture.image.toString('base64'), mime: 'image/png',
    }]);
    await expect(await $(`button=Remove selected ${fixture.imageTarget}`)).toExist();
    await selectedImage();
    await renderedImage();
    await exactOriginal();
    await (await $('button=Back')).click();
    await (await $('button=Library')).click();
    await expect(await $(documentButton)).toHaveText(/Pinned/);
  });

  it('retains original bytes, selected image, pin and appearance across a real app restart', async () => {
    await (await $('button=Settings')).click();
    for (const choice of ['System', 'Light', 'Dark']) {
      await expect(await $(`//label[normalize-space(.)="${choice}"]/input[@type="radio"]`)).toExist();
    }
    await (await $('//label[normalize-space(.)="Dark"]')).click();
    await expect(await $('html[data-theme="dark"]')).toExist();
    await (await $('button=Library')).click();
    // The product debounces appearance saves by 100 ms. Relaunch assertions below
    // verify durability; this is not a crash-during-write acceptance claim.
    await browser.pause(500);
    await browser.switchContext('NATIVE_APP');
    await browser.terminateApp(E2E_BUNDLE_IDENTIFIER);
    await browser.activateApp(E2E_BUNDLE_IDENTIFIER);
    await switchToMobileWebview();
    await expect(await $('html[data-theme="dark"]')).toExist();
    await expect(await $(documentButton)).toHaveText(/Pinned/);
    const library = await nativeRead('mobile_reader_list');
    await expect(library.documents.find((value) => value.id === imported.id)?.pinned).toBe(true);
    await exactOriginal();
    await selectedImage();
    await (await $(documentButton)).click();
    await expect(await $('main[aria-label="Reader"] h1')).toHaveText('Offline reading handbook');
    await renderedImage();
  });
});
