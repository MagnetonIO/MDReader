/** Private opaque retry identity only: no title, path, body, token or key. */
import type { SelectedOperation, SelectedScope } from '../../bindings';
const KEY = 'mdreader.selected-sync.v1';
const MAX_BYTES = 16_384;
const MAX_SCOPES = 8;
const ID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const DEVICE = /^[A-Za-z0-9_-]{42}[AEIMQUYcgkosw048]$/;
const HASH = /^[0-9a-f]{64}$/;
const fault = () => new Error('The retained upload identity is unavailable or unsupported. It was not replaced. Local reading remains available.');
function object(value: unknown, keys: string[]): value is Record<string, unknown> {
  return !!value && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}
function text(value: unknown, pattern: RegExp): value is string { return typeof value === 'string' && pattern.test(value); }
export function validSelection(value: unknown): value is SelectedOperation {
  if (!object(value, ['scope', 'receipt', 'operationId']) || !text(value.operationId, ID)
    || !object(value.scope, ['accountDigest', 'libraryId', 'deviceId'])
    || !object(value.receipt, ['documentId', 'sourceId', 'revision'])) return false;
  return text(value.scope.accountDigest, HASH) && text(value.scope.libraryId, ID) && text(value.scope.deviceId, DEVICE)
    && text(value.receipt.documentId, ID) && text(value.receipt.sourceId, HASH) && text(value.receipt.revision, HASH);
}
export function sameScope(a: SelectedScope, b: SelectedScope): boolean {
  return a.accountDigest === b.accountDigest && a.libraryId === b.libraryId && a.deviceId === b.deviceId;
}
export function sameSelection(a: SelectedOperation, b: SelectedOperation): boolean {
  return sameScope(a.scope, b.scope) && a.operationId === b.operationId && a.receipt.documentId === b.receipt.documentId
    && a.receipt.sourceId === b.receipt.sourceId && a.receipt.revision === b.receipt.revision;
}
interface Pending { version: 1; operations: SelectedOperation[] }
function read(storage: Storage): { raw: string | null; value: Pending } {
  const raw = storage.getItem(KEY);
  if (raw === null) return { raw, value: { version: 1, operations: [] } };
  if (raw.length > MAX_BYTES || new TextEncoder().encode(raw).length > MAX_BYTES) throw fault();
  let value: unknown;
  try { value = JSON.parse(raw); } catch { throw fault(); }
  // Our machine-owned canonical format deliberately admits no duplicate members,
  // alternate encodings or unknown fields. Preserve every rejected original byte.
  if (!object(value, ['version', 'operations']) || value.version !== 1 || !Array.isArray(value.operations)
    || value.operations.length > MAX_SCOPES || !value.operations.every(validSelection)
    || JSON.stringify(value) !== raw) throw fault();
  const operations = value.operations;
  if (operations.some((op, index) => operations.slice(index + 1).some((other) => sameScope(op.scope, other.scope)))) throw fault();
  return { raw, value: { version: 1, operations } };
}
export function pendingSelections(storage: Storage = localStorage): SelectedOperation[] {
  try { return read(storage).value.operations; } catch { throw fault(); }
}
function update(storage: Storage, change: (operations: SelectedOperation[]) => SelectedOperation[]): void {
  const before = read(storage);
  const operations = change(before.value.operations);
  if (operations.length > MAX_SCOPES) throw fault();
  const next = JSON.stringify({ version: 1, operations });
  if (next.length > MAX_BYTES || storage.getItem(KEY) !== before.raw) throw fault();
  if (next === before.raw) return; // Polled native metadata is often unchanged.
  storage.setItem(KEY, next);
  if (storage.getItem(KEY) !== next) throw fault();
}
async function locked(action: () => void): Promise<void> {
  if (!navigator.locks?.request) throw new Error('This reader cannot safely retain an upload across windows. Selected upload is unavailable; local reading remains available.');
  try {
    await navigator.locks.request(KEY, { mode: 'exclusive', ifAvailable: true }, (lock) => {
      if (!lock) throw new Error('Another window is updating retained uploads. Try again after it finishes.');
      // Synchronous bounded storage only. Never hold this origin lock over IPC,
      // account work or network waits. Every cooperating writer uses this lock.
      action();
    });
  } catch (cause) { throw cause instanceof Error ? cause : fault(); }
}
export async function retainSelection(selection: SelectedOperation, storage: Storage = localStorage): Promise<void> {
  if (!validSelection(selection)) throw fault();
  await locked(() => {
    try {
      update(storage, (operations) => {
        const previous = operations.find((op) => sameScope(op.scope, selection.scope));
        if (previous && !sameSelection(previous, selection)) throw fault();
        return previous ? operations : [...operations, selection];
      });
    } catch { throw fault(); }
  });
}
/** Call only with the exact same-actor absent/applied disposition, never counts. */
export async function settleSelection(selection: SelectedOperation, storage: Storage = localStorage): Promise<void> {
  await locked(() => {
    try {
      update(storage, (operations) => {
        const previous = operations.find((op) => sameScope(op.scope, selection.scope));
        if (!previous || !sameSelection(previous, selection)) throw fault();
        return operations.filter((op) => op !== previous);
      });
    } catch { throw fault(); }
  });
}
