/**
 * Native mobile smoke over an Appium-provided WEBVIEW context.
 *
 * This deliberately checks the mobile composition instead of replaying the
 * desktop document flow: mobile currently has no durable folder grant or
 * secure cloud sign-in, so manufacturing seeded documents would turn a
 * scaffold into false native product evidence.
 */

import { $, browser, expect } from '@wdio/globals';

/** Switch the Appium session into the app's WEBVIEW context (the HTML UI). */
async function switchToWebview(b: WebdriverIO.Browser): Promise<void> {
  const deadline = Date.now() + 30_000;
  for (;;) {
    const contexts = (await b.getContexts()) as unknown as string[];
    const webview = contexts.find((context) =>
      String(context).toUpperCase().includes('WEBVIEW'),
    );
    if (webview) {
      await b.switchContext(webview);
      return;
    }
    if (Date.now() >= deadline) {
      throw new Error(`no WEBVIEW context appeared (contexts: ${JSON.stringify(contexts)})`);
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
}

describe('MDReader native mobile shell', () => {
  before(async () => switchToWebview(browser));

  it('mounts the four mobile tabs without desktop chrome', async () => {
    const navigation = await $('nav[aria-label="Primary"]');
    await expect(navigation).toHaveText('LibrarySearchSourcesSettings');
    await expect(await $('button=Source')).not.toExist();
    await expect(await $('button=Split')).not.toExist();
    await expect(await $('button*=Check for updates')).not.toExist();
  });

  it('shows only truthful mobile source boundaries', async () => {
    await (await $('button=Sources')).click();
    await expect(await $('h1=Sources')).toExist();
    await expect(await $('h2=Device folders')).toExist();
    await expect(
      await $('p=Unavailable until a durable mobile folder grant is implemented'),
    ).toExist();
    await expect(await $('button=Add folder')).not.toExist();
    await expect(await $('h2=Google Drive')).toExist();
    await expect(await $('h2=Dropbox')).toExist();
    await expect(await $('h2=Enterprise CatDB server')).toExist();
    await expect(await $('input[type="password"]')).not.toExist();
  });

  it('preserves System, Light, and Dark theme controls', async () => {
    await (await $('button=Settings')).click();
    for (const theme of ['System', 'Light', 'Dark']) {
      await expect(await $(`span=${theme}`)).toExist();
    }
    await (await $('span=Dark')).click();
    await expect(await $('html[data-theme="dark"]')).toExist();
  });
});
