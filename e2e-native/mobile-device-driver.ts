import { browser } from '@wdio/globals';
import type { AssetReply, DeviceLibrary, DeviceRead } from '../ui/src/bindings.ts';

type NativeReplies = {
  mobile_reader_list: DeviceLibrary;
  mobile_reader_read: DeviceRead;
  mobile_reader_assets: AssetReply;
};
type NativeResult<T> = { ok: true; value: T } | { ok: false; error: string };

/** Observes the real native owner. Mutations still go through the product UI. */
export async function nativeReply<K extends keyof NativeReplies>(
  command: K, args: Record<string, unknown> = {},
): Promise<NativeResult<NativeReplies[K]>> {
  const serialized = await browser.executeAsync<string, [string, Record<string, unknown>]>((nativeCommand, nativeArgs, done) => {
    const bridge = (window as Window & {
      __TAURI_INTERNALS__?: { invoke: (name: string, args: Record<string, unknown>) => Promise<unknown> };
    }).__TAURI_INTERNALS__;
    if (!bridge) {
      done(JSON.stringify({ ok: false, error: 'No native Tauri IPC bridge' }));
      return;
    }
    // WebKit's synchronous WebDriver execute does not await the returned IPC
    // Promise. The async callback makes completion explicit for both drivers.
    Promise.resolve().then(() => bridge.invoke(nativeCommand, nativeArgs)).then(
      (value) => done(JSON.stringify({ ok: true, value })),
      (cause: unknown) => done(JSON.stringify({ ok: false, error: String(cause) })),
    );
  }, command, args);
  // WebKit's WebDriver marshaller treats objects containing a `document` member
  // as Window references. Serialize inside the webview to preserve the exact
  // native DTO, then validate the bounded envelope on the harness side.
  if (typeof serialized !== 'string' || serialized.length > 32 * 1024 * 1024)
    throw new Error('Invalid native bridge reply envelope');
  let reply: NativeResult<unknown>;
  try { reply = JSON.parse(serialized); }
  catch { throw new Error('Invalid native bridge reply envelope'); }
  if (!reply || typeof reply !== 'object' || typeof reply.ok !== 'boolean'
    || reply.ok && !Object.hasOwn(reply, 'value')
    || (!reply.ok && typeof reply.error !== 'string'))
    throw new Error('Invalid native bridge reply envelope');
  return reply as NativeResult<NativeReplies[K]>;
}

export async function nativeRead<K extends keyof NativeReplies>(
  command: K, args: Record<string, unknown> = {},
): Promise<NativeReplies[K]> {
  const reply = await nativeReply(command, args);
  if (!reply.ok) throw new Error(`${command}: ${reply.error}`);
  return reply.value;
}

/** File selection through the real DOM input/onChange pipeline, not a seeded
 * database or mocked adapter. Native OS picker interactions are a separate gate. */
export async function chooseFiles(selector: string, files: Array<{
  name: string; base64: string; mime: string;
}>): Promise<void> {
  await browser.execute((inputSelector, selected) => {
    const input = document.querySelector<HTMLInputElement>(inputSelector);
    if (!input || input.type !== 'file' || input.disabled)
      throw new Error('The product file input is unavailable');
    const transfer = new DataTransfer();
    for (const file of selected) {
      const bytes = Uint8Array.from(atob(file.base64), (character) => character.charCodeAt(0));
      transfer.items.add(new File([bytes], file.name, { type: file.mime }));
    }
    input.files = transfer.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
  }, selector, files);
}

/** Reacquire the new webview after an actual native app termination/relaunch. */
export async function switchToMobileWebview(): Promise<void> {
  await browser.switchContext('NATIVE_APP');
  const deadline = Date.now() + 30_000;
  do {
    const contexts = await browser.getContexts() as unknown as string[];
    const webview = contexts.find((context) => typeof context === 'string' && /WEBVIEW/i.test(context));
    if (webview) {
      await browser.switchContext(webview);
      await browser.$('nav[aria-label="Primary"]').waitForExist();
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  } while (Date.now() < deadline);
  throw new Error('No MDReader WEBVIEW appeared within 30 seconds');
}
