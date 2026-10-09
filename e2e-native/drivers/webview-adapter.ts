/**
 * Milestone PT — the generic WEBVIEW adapter.
 *
 * Every platform whose webview exposes a WebDriver bridge shares this adapter: the
 * desktop tauri-driver legs (Linux/WebKitGTK, Windows/WebView2) and the mobile
 * Appium legs (iOS via appium-xcuitest-driver, Android via
 * appium-uiautomator2-driver) after switching into their WEBVIEW context. It drives
 * the SAME `flows.ts` off the live DOM, reading the very class names the shared
 * spec is expressed in (`.filetree__row`, `.mdsearch__item`, `.mdsearch`, …).
 *
 * These sessions drive the webview DOCUMENT, not the OS window chrome, so
 * {@link PlatformDriver.windowOps} is false: the runner registers the native-window
 * steps (drag / resize / red-X close) PENDING with {@link windowPendingReason}.
 * Those live in the macOS mac2 leg, which CAN move and close the real window.
 *
 * `activate()` runs once before the flow — a no-op on desktop (the webview is the
 * only context), and the WEBVIEW-context switch on mobile.
 */

import type { Chord } from '../flows.ts';
import { SELECTORS } from '../flows.ts';
import type { DragOrigin, FrameToken, PlatformDriver, Rect } from './types.ts';

/** The primary modifier the app listens for off-macOS (detectMac === false). */
const MOD = 'Control';

export interface WebviewAdapterOptions {
  /** The live WebdriverIO session (tauri-driver or Appium mobile). */
  browser: WebdriverIO.Browser;
  /** Human label for logs, e.g. "Linux (tauri-driver / WebKitGTK)". */
  name: string;
  /** Why the native-window steps are pending on this leg. */
  windowPendingReason: string;
  /** Optional one-time context activation (mobile WEBVIEW-context switch). */
  activate?: (browser: WebdriverIO.Browser) => Promise<void>;
}

export class WebviewAdapter implements PlatformDriver {
  readonly name: string;
  readonly windowOps = false;
  readonly windowPendingReason: string;
  readonly frameOracle = false;
  private readonly b: WebdriverIO.Browser;
  private readonly activateFn?: (browser: WebdriverIO.Browser) => Promise<void>;
  private activated = false;

  constructor(o: WebviewAdapterOptions) {
    this.b = o.browser;
    this.name = o.name;
    this.windowPendingReason = o.windowPendingReason;
    this.activateFn = o.activate;
  }

  private async ensureActivated(): Promise<void> {
    if (this.activated) return;
    if (this.activateFn) await this.activateFn(this.b);
    this.activated = true;
  }

  async focus(): Promise<void> {
    await this.ensureActivated();
  }

  async dispose(): Promise<void> {
    // The wdio testrunner owns the session lifecycle; nothing to release here.
  }

  // ---- WEBVIEW reads (live DOM) ----

  async shellMounted(): Promise<boolean> {
    await this.ensureActivated();
    return this.b.$(SELECTORS.workspaceTree.css!).isExisting();
  }

  async countClass(cssClass: string): Promise<number> {
    await this.ensureActivated();
    const els = await this.b.$$(`.${cssClass}`);
    return els.length;
  }

  async statusText(): Promise<string> {
    await this.ensureActivated();
    return this.b.$(SELECTORS.statusBar.css!).getText();
  }

  async headings(): Promise<string> {
    await this.ensureActivated();
    const els = await this.b.$$(`${SELECTORS.reader.css!} h1, ${SELECTORS.reader.css!} h2, ${SELECTORS.reader.css!} h3`);
    const parts: string[] = [];
    for (const el of els) parts.push(await el.getText());
    // Fall back to the whole reader text if no heading tags surfaced.
    return parts.length ? parts.join('\n') : this.readerText();
  }

  async readerText(): Promise<string> {
    await this.ensureActivated();
    return this.b.$(SELECTORS.reader.css!).getText();
  }

  async paletteContainerCount(): Promise<number> {
    await this.ensureActivated();
    const els = await this.b.$$('.mdsearch');
    return els.length;
  }

  async scopedText(cssClass: string): Promise<string> {
    await this.ensureActivated();
    return this.b.$(`.${cssClass}`).getText();
  }

  async clickIn(cssClass: string, contains?: string): Promise<void> {
    await this.ensureActivated();
    const elements = await this.b.$$(`.${cssClass}`);
    for (const element of elements) {
      if (contains === undefined || (await element.getText()).includes(contains)) {
        await element.click();
        return;
      }
    }
    throw new Error(`No .${cssClass} control matches the requested text`);
  }

  // ---- input ----

  async chord(chord: Chord): Promise<void> {
    await this.b.keys(this.chordKeys(chord));
  }

  async focusInput(cssClass: string): Promise<void> {
    await this.ensureActivated();
    await this.b.$(`.${cssClass}`).click();
  }

  async inputValue(cssClass: string): Promise<string> {
    await this.ensureActivated();
    return this.b.$(`.${cssClass}`).getValue();
  }

  async type(text: string): Promise<void> {
    await this.b.keys([...text]);
  }

  private chordKeys(chord: Chord): string[] {
    if (chord === 'Enter') return ['Enter'];
    if (chord === 'Escape') return ['Escape'];
    const parts = chord.split('+');
    const key = parts[parts.length - 1]!.toLowerCase();
    return parts.includes('Mod') ? [MOD, key] : [key];
  }

  // ---- NATIVE window: not drivable through a webview WebDriver session ----

  async isWindowPresent(): Promise<boolean> {
    return this.shellMounted();
  }

  async isProcessAlive(): Promise<boolean> {
    return this.windowUnsupported();
  }

  async windowRect(): Promise<Rect> {
    return this.windowUnsupported();
  }

  async dragBy(_origin: DragOrigin, _dx: number, _dy: number): Promise<void> {
    return this.windowUnsupported();
  }

  async resizeBy(_dw: number, _dh: number): Promise<Rect> {
    return this.windowUnsupported();
  }

  async clickClose(): Promise<void> {
    return this.windowUnsupported();
  }

  private windowUnsupported(): never {
    throw new Error(
      `native-window op called on a webview-only driver (${this.name}); the runner should have ` +
        `registered this step pending — ${this.windowPendingReason}`,
    );
  }

  // No frame oracle on the DOM legs — the DOM assertions are already rigorous.
  captureFrame?: () => Promise<FrameToken>;
  assertFrameChanged?: (before: FrameToken, label: string) => Promise<number>;
}
