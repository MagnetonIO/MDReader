type Priority = 'foreground' | 'metadata';
class ProviderQueueBusy extends Error { readonly kind = 'busy'; }
interface Pending { priority: Priority; run(): Promise<void> }

/** Transport admission only. Native still verifies the exact grant/receipt.
 * Finish one bounded batch, then give explicit reads/edits precedence over discovery. */
export class ProviderRequestQueue {
  private active = false;
  private pending: Pending[] = [];
  run<T>(priority: Priority, operation: () => Promise<T>): Promise<T> {
    if (this.pending.length >= 32) return Promise.reject(new ProviderQueueBusy('Provider request queue is busy. Try again shortly.'));
    return new Promise((resolve, reject) => {
      this.pending.push({ priority, run: async () => {
        try { resolve(await operation()); } catch (cause) { reject(cause); }
      } });
      this.pump();
    });
  }
  private pump() {
    if (this.active || !this.pending.length) return;
    const foreground = this.pending.findIndex(item => item.priority === 'foreground');
    const next = this.pending.splice(foreground < 0 ? 0 : foreground, 1)[0]!;
    this.active = true;
    void next.run().finally(() => { this.active = false; this.pump(); });
  }
}

/** One adapter queue shared by native provider reads and guarded writes. */
export const providerRequestQueue = new ProviderRequestQueue();
