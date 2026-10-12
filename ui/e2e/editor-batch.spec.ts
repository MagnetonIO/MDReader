import type { E2EHooks } from '../src/e2e/harness';
import { test, expect, type Page } from '@playwright/test';
async function modifier(page: Page) {
  return page.evaluate(() => /mac|iphone|ipad|ipod/i.test((navigator as unknown as { userAgentData?: { platform?: string } }).userAgentData?.platform ?? navigator.platform) ? 'Meta' : 'Control');
}
async function openEditor(page: Page, name = 'reference.md') {
  await page.goto('/e2e.html');
  await page.getByRole('button', { name: 'Open folder…' }).click();
  await page.getByRole('treeitem', { name }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(page.locator('.cm-content')).toBeVisible();
}
test('unsaved Preview/Split preserve source, save mode and shared font size', async ({ page }) => {
  await openEditor(page);
  const mod = await modifier(page), source = page.locator('.cm-content');
  await source.evaluate(el => el.setAttribute('data-acceptance-instance', 'first'));
  await source.click(); await page.keyboard.press(`${mod}+A`);
  await page.keyboard.insertText('# Unsaved preview\n\nalpha beta');
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(page.locator('.reader-host').getByRole('heading', { name: /Unsaved preview/ })).toBeVisible();
  await expect(page.locator('.reader-host').getByRole('heading', { name: /^Reference/ })).toHaveCount(0);
  await expect(source).toBeHidden();
  const sentBefore = await page.evaluate(() => (window as unknown as { __E2E__: E2EHooks }).__E2E__.sent().filter((item) => item.cmd === 'save_doc').length);
  expect(sentBefore).toBe(0);
  await page.getByRole('button', { name: 'Split', exact: true }).click();
  await expect(source).toHaveAttribute('data-acceptance-instance', 'first');
  const sourceSize = await source.evaluate(el => parseFloat(getComputedStyle(el).fontSize));
  const chrome = page.locator('.editor-toolbar');
  const chromeSize = await chrome.evaluate(el => getComputedStyle(el).fontSize);
  await source.click(); await page.keyboard.press(`${mod}+=`);
  await expect.poll(() => source.evaluate(el => parseFloat(getComputedStyle(el).fontSize))).toBeGreaterThan(sourceSize);
  await expect.poll(() => chrome.evaluate(el => getComputedStyle(el).fontSize)).toBe(chromeSize);
  await page.keyboard.press(`${mod}+s`);
  await expect(page.locator('.editor-toolbar__status')).toHaveText('Saved');
  await expect(page.getByRole('button', { name: 'Split', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(source).toHaveAttribute('data-acceptance-instance', 'first');
  await page.setViewportSize({ width: 650, height: 750 });
  await expect(page.getByRole('button', { name: 'Split', exact: true })).toHaveCount(0);
  await page.setViewportSize({ width: 1200, height: 800 });
  await expect(page.getByRole('button', { name: 'Split', exact: true })).toHaveAttribute('aria-pressed', 'true');
});
test('real CodeMirror find/replace, formatting undo and link chord stay in the editor', async ({ page }) => {
  await openEditor(page);
  const mod = await modifier(page), source = page.locator('.cm-content');
  await source.click(); await page.keyboard.press(`${mod}+A`); await page.keyboard.insertText('alpha alpha');
  await page.keyboard.press(`${mod}+f`);
  await expect(page.locator('.cm-search input[name=search]')).toBeVisible();
  await page.locator('.cm-search input[name=search]').fill('alpha');
  await page.locator('.cm-search input[name=replace]').fill('beta');
  await page.locator('.cm-search button[name=replaceAll]').click();
  await expect(source).toHaveText('beta beta');
  await page.keyboard.press('Escape'); await source.click(); await page.keyboard.press(`${mod}+A`);
  await page.keyboard.press(`${mod}+b`); await expect(source).toHaveText('**beta beta**');
  await page.keyboard.press(`${mod}+z`); await expect(source).toHaveText('beta beta');
  await page.keyboard.press(`${mod}+A`); await page.keyboard.press(`${mod}+k`);
  await expect(source).toHaveText('[beta beta](url)');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});
test('changing documents never carries another file into undo or save', async ({ page }) => {
  await openEditor(page);
  const mod = await modifier(page), source = page.locator('.cm-content');
  await source.click(); await page.keyboard.press(`${mod}+A`); await page.keyboard.insertText('first file only');
  await page.getByRole('treeitem', { name: 'guide.md' }).click();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await source.click(); await page.keyboard.press(`${mod}+A`); await page.keyboard.insertText('guide edit');
  await page.keyboard.press(`${mod}+z`); await expect(source).toContainText('# Guide');
  await page.keyboard.press(`${mod}+z`); await expect(source).not.toContainText('first file only');
});
