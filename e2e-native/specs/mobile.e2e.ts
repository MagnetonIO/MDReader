/**
 * Native mobile product flow over an Appium-provided WEBVIEW context.
 *
 * The source is a loopback-only server fixture under the isolated `.e2e`
 * application identity. It exercises the shipping authenticated adapter and
 * offline cache; it is test evidence, never a production-provider claim.
 */

import { $, browser, expect } from '@wdio/globals';

import { MOBILE_NAVIGATION_TEXT_PATTERN } from '../mobile-assertions.ts';
import { MOBILE_FIXTURE_IDS } from '../mobile-fixture.ts';

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

describe('MDReader native mobile product flow', () => {
  before(async () => switchToWebview(browser));

  it('smoke: mounts the four mobile tabs without desktop chrome', async () => {
    const navigation = await $('nav[aria-label="Primary"]');
    await expect(navigation).toHaveText(MOBILE_NAVIGATION_TEXT_PATTERN);
    await expect(await $('button=Source')).not.toExist();
    await expect(await $('button=Split')).not.toExist();
    await expect(await $('button*=Check for updates')).not.toExist();
  });

  it('admits the isolated Enterprise fixture while keeping other source boundaries truthful', async () => {
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
    await expect(await $('input[type="url"]')).not.toExist();
    await expect(await $('button=Use server')).not.toExist();
    await expect(await $('p*=Connected to Enterprise Document library')).toExist();
    await (await $('button=Library')).click();
    await expect(await $('button*=mobile-e2e.md')).toExist();
  });

  it('searches the admitted title, reads it, and inspects versioned provenance', async () => {
    await (await $('button=Search')).click();
    await (await $('input[type="search"]')).setValue('Mobile evidence handbook');
    await (await $('button*=mobile-e2e.md')).click();
    await expect(await $('h1=Mobile evidence handbook')).toExist();
    await expect(await $('main[aria-label="Reader"]')).toExist();
    await (await $('button=Evidence')).click();
    await expect(await $('main[aria-label="Semantic Inspector"]')).toExist();
    await expect(await $(`dd*=${MOBILE_FIXTURE_IDS.source}`)).toExist();
    for (const value of [
      MOBILE_FIXTURE_IDS.catalog,
      MOBILE_FIXTURE_IDS.schema,
      MOBILE_FIXTURE_IDS.mapping,
      MOBILE_FIXTURE_IDS.policy,
    ]) {
      await expect(await $(`dd=${value}`)).toExist();
    }
    await expect(await $('dd*=sha256:')).toExist();
  });

  it('uses the identity-authorized offline copy when the fixture read becomes unavailable', async () => {
    await (await $('button=Back')).click();
    await (await $('button=Library')).click();
    await (await $('button*=mobile-e2e.md')).click();
    await expect(await $('h1=Mobile evidence handbook')).toExist();
    await expect(await $('aside*=Offline copy')).toExist();
    await expect(await $(`aside*=${MOBILE_FIXTURE_IDS.deployment}`)).toExist();
    await expect(await $('aside*=sha256:')).toExist();
  });

  it('preserves System, Light, and Dark theme controls', async () => {
    await (await $('button=Back')).click();
    await (await $('button=Settings')).click();
    for (const theme of ['System', 'Light', 'Dark']) {
      await expect(await $(`span=${theme}`)).toExist();
    }
    await (await $('span=Dark')).click();
    await expect(await $('html[data-theme="dark"]')).toExist();
  });
});
