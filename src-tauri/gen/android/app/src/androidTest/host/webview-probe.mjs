/** Test-only CDP transport to one debug WebView in an already running Android
 * receiver PID. Never starts or instruments the receiver process. */
export async function connectReceiver(adb, pid) {
  if (!/^[1-9]\d{0,8}$/.test(pid)) throw new Error('Receiver PID is invalid.');
  const socket = `webview_devtools_remote_${pid}`;
  const sockets = adb(['shell', 'cat', '/proc/net/unix']);
  if (!sockets.split('\n').some((line) => line.trim().endsWith(`@${socket}`)))
    throw new Error('The selected receiver has no debug WebView socket.');
  const port = adb(['forward', 'tcp:0', `localabstract:${socket}`]).trim();
  if (!/^\d{1,5}$/.test(port)) throw new Error('Debug WebView forward is unavailable.');
  let web;
  const remove = () => { try { adb(['forward', '--remove', `tcp:${port}`]); } catch {} };
  try {
    const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(5000) });
    const targets = await response.json();
    const pages = targets.filter((target) => {
      if (target.type !== 'page') return false;
      const url = new URL(target.url);
      return ['http:', 'https:'].includes(url.protocol) && ['tauri.localhost', 'localhost'].includes(url.hostname);
    });
    if (pages.length !== 1) throw new Error('One MDReader debug page is required.');
    const remote = new URL(pages[0].webSocketDebuggerUrl);
    if (!/^\/devtools\/page\/[a-zA-Z0-9_-]{1,128}$/.test(remote.pathname)) throw new Error('Debug page identity is invalid.');
    web = new WebSocket(`ws://127.0.0.1:${port}${remote.pathname}`);
    await new Promise((resolve, reject) => {
      const timeout = setTimeout(() => reject(new Error('Debug WebView connection timed out.')), 5000);
      web.addEventListener('open', () => { clearTimeout(timeout); resolve(); }, { once: true });
      web.addEventListener('error', () => { clearTimeout(timeout); reject(new Error('Debug WebView connection failed.')); }, { once: true });
    });
    let next = 1;
    const pending = new Map();
    web.addEventListener('message', (message) => {
      let value;
      try { value = JSON.parse(message.data); } catch { return; }
      const request = pending.get(value.id);
      if (!request) return;
      pending.delete(value.id); clearTimeout(request.timer);
      if (value.error || value.result?.exceptionDetails) request.reject(new Error('Receiver probe failed.'));
      else request.resolve(value.result?.result?.value);
    });
    return {
      evaluate(expression) {
        return new Promise((resolve, reject) => {
          const id = next++;
          const timer = setTimeout(() => { pending.delete(id); reject(new Error('Receiver probe timed out.')); }, 40_000);
          pending.set(id, { resolve, reject, timer });
          web.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
        });
      },
      close() {
        for (const request of pending.values()) { clearTimeout(request.timer); request.reject(new Error('Receiver probe closed.')); }
        pending.clear(); web.close(); remove();
      },
    };
  } catch {
    web?.close(); remove(); throw new Error('The debug receiver could not be inspected.');
  }
}
