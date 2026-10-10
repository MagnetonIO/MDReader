import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import App from "./App";
import { useSettingsStore } from "./stores/settings";
import { setTestPlatform } from "./test/tauri-os";
import { DEVICE_READ_CHANGED } from "./features/device-reader/changed";
import { readingLink } from "./features/document-transfer/reading-link";

const persistence = vi.hoisted(() => ({
  stores: new Map<string, Map<string, unknown>>(),
}));
let nativeDeviceConfigured = false;
let nativeProviderConnected = false;
let pendingImports: { id: string; name: string; bytes: number }[] = [];
let rejectDiscard = false;
let pinAcknowledgement:Promise<void>|null=null;
let readFailure: string | null = null;
let positionAcknowledgement: Promise<void> | null = null;
let manifestAcknowledgement: Promise<void> | null = null;
let extraDocument = false;
let deviceMarkdown = "# Semantic architecture\n\nProvenance-aware reading.";
const deviceDocument = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "architecture.md",
  source_id: "b".repeat(64),
  revision: "a".repeat(64),
  bytes: 128,
  pinned: false,
  line: 1,
  indexed: true,
};
const secondDocument = { ...deviceDocument, id: "33333333-3333-4333-8333-333333333333", name: "second.md", source_id: "c".repeat(64) };
const nativeCalls = vi.fn();
const providerSourceId = 'e'.repeat(64);
const providerProjection = { jobId: 'provider-fixture', readerSourceId: `provider:${providerSourceId}`, path: `records/${'f'.repeat(64)}.md`, revision: 'd'.repeat(64) };

vi.mock("@tauri-apps/plugin-store", () => ({
  load: async (name: string) => ({
    get: async (key: string) => persistence.stores.get(name)?.get(key) ?? null,
    set: async (key: string, value: unknown) => {
      let store = persistence.stores.get(name);
      if (!store) {
        store = new Map();
        persistence.stores.set(name, store);
      }
      store.set(key, value);
    },
    save: async () => undefined,
  }),
}));

function installTauriRuntime(): void {
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    configurable: true,
    value: {
      invoke: vi.fn(async (command: string, args?: Record<string, unknown>) => {
        nativeCalls(command, args);
        if (command === 'mobile_reader_import_incoming') return { document: null, remaining: 0, error: null, retained: false, open: false };
        if (command === "native_profile_persistence_host") return "legacy_host";
        if (command === 'native_provider_status') return {
          availability: ['google_drive', 'dropbox'].map(provider => ({ provider, configured: nativeProviderConnected, supported: true,
            diagnostic: nativeProviderConnected ? null : 'Provider registration is not configured in this build.' })),
          sources: nativeProviderConnected ? [{ sourceId: providerSourceId, readerSourceId: providerProjection.readerSourceId, name: 'Selected Drive notes', kind: 'google_drive', enabled: true,
            selectedScope: { kind: 'exact_documents', count: 1 }, capabilities: { read: true, list: true, changes: false, assets: false, guardedWrite: false },
            health: 'ready', coverage: { kind: 'complete_selected', observedAtMs: 1 } }] : [],
          pendingAdmissions: [], error: null,
        };
        if (command === 'native_provider_request') {
          const request = args?.request as { operation: string; sourceId: string; path?: string; revision?: string };
          if (request.operation === 'documents') return { operation: 'documents', offset: 0, nextOffset: null, documents: [{ documentId: 'cloud-note', name: 'cloud-note.md', displayPath: '/Notes/cloud-note.md',
            modifiedAtMs: 1, sizeBytes: 60, cached: true, cachedObservedAtMs: 1, admitted: true, projection: providerProjection }] };
          if (request.operation === 'read_cached') return { operation: 'read_cached', projection: providerProjection };
          if (request.operation === 'read_document') return { operation: 'read_document', sourceKind: 'provider', markdown: '# Cached cloud note\n\nOwned selected provider revision.',
            sourceId: request.sourceId, path: request.path, revision: request.revision, lineStart: 1, lineEnd: 3 };
          throw new Error(`Unexpected provider operation: ${request.operation}`);
        }
        if (command === 'mobile_provider_reading_state') {
          const request = args?.request as { reference: { sourceId: string; path: string; revision: string } };
          return { reference: request.reference, pinned: false, line: 1, lineEnd: 3, generation: '0' };
        }
        if (command === 'mobile_reader_transfer_capabilities') return { export: nativeProviderConnected, share: nativeProviderConnected, readingLinks: false };
        if (command === 'mobile_provider_transfer') return { state: args?.action === 'share' ? 'presented' : 'exported' };
        if (command === "mobile_reader_assets") {
          const request = args?.request as { operation: string; document_id?: string; revision?: string };
          if (request.operation === "manifest") { await manifestAcknowledgement; return { operation: "manifest", value: {
            document_id: request.document_id, document_revision: request.revision,
            manifest: "b".repeat(64), assets: [], pending: [], targets: [], targets_truncated: false,
          } }; }
          if (request.operation === "inventory") return { operation: "inventory", value: {
            items: [], reserved_bytes: 0, max_bytes: 32 * 1024 * 1024, max_items: 512,
          } };
          throw new Error(`Unexpected asset operation: ${request.operation}`);
        }
        if (command === "mobile_reader_list")
          return {documents: nativeDeviceConfigured ? [{ ...deviceDocument }, ...(extraDocument ? [{ ...secondDocument }] : [])] : [], pending: [...pendingImports]};
        if (command === "mobile_reader_discard_import") {
          if (rejectDiscard) throw new Error("Incomplete copy could not be removed");
          pendingImports = [];
          return null;
        }
        if (command === "mobile_reader_read") {
          if (readFailure) throw new Error(readFailure);
          return {
            document: { ...(args?.id === secondDocument.id ? secondDocument : deviceDocument) },
            markdown: args?.id === secondDocument.id ? "# Second selection\n\n## Section two" : deviceMarkdown,
          };
        }
        if (command === "mobile_search_settings") return {
          enabled: false, model: "not_downloaded", documents: nativeDeviceConfigured ? 1 : 0, embedded_documents: 0,
        };
        if (command === "mobile_search_query")
          return { semantic_unavailable: null, hits: [
            {
              document: { ...deviceDocument },
              line: 3,
              text: "Provenance-aware reading.",
            },
          ] };
        if (command === "reader_account") return { operation: "status", status: {
          configured: false, connected: false, retainedLibrary: false, custodyWritePending: false, activeLibraryId: null, existingLibraryId: null, existingLibrarySameSelection: false, recoveryAvailable: false,
          custody: null, selectedScope: null, latestJob: null,
        } };
        if (command === "mobile_reader_pin") {
          await pinAcknowledgement;
          deviceDocument.pinned = Boolean(args?.pinned);
          return null;
        }
        if (command === "mobile_reader_position") { await positionAcknowledgement; return null; }
        if (command === "mobile_reader_import") {
          deviceMarkdown = String(args?.content);
          deviceDocument.name = String(args?.name);
          nativeDeviceConfigured = true;
          return { ...deviceDocument };
        }
        throw new Error(`Unexpected mobile command: ${command}`);
      }),
    },
  });
}

let viewportWidth = 390;
let systemDark = false;
const mediaListeners = new Map<
  string,
  Set<(event: MediaQueryListEvent) => void>
>();

function matches(query: string): boolean {
  if (query === "(min-width: 768px)") return viewportWidth >= 768;
  if (query === "(prefers-color-scheme: dark)") return systemDark;
  return false;
}

function installMatchMedia(): void {
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    value: vi.fn((query: string) => {
      const listeners = mediaListeners.get(query) ?? new Set();
      mediaListeners.set(query, listeners);
      return {
        get matches() {
          return matches(query);
        },
        media: query,
        onchange: null,
        addEventListener: (
          _type: string,
          listener: EventListenerOrEventListenerObject,
        ) => {
          if (typeof listener === "function") {
            listeners.add(listener as (event: MediaQueryListEvent) => void);
          }
        },
        removeEventListener: (
          _type: string,
          listener: EventListenerOrEventListenerObject,
        ) => {
          if (typeof listener === "function") {
            listeners.delete(listener as (event: MediaQueryListEvent) => void);
          }
        },
        addListener: (listener: (event: MediaQueryListEvent) => void) =>
          listeners.add(listener),
        removeListener: (listener: (event: MediaQueryListEvent) => void) =>
          listeners.delete(listener),
        dispatchEvent: () => true,
      } as MediaQueryList;
    }),
  });
}

function setViewport(width: number): void {
  viewportWidth = width;
  Object.defineProperty(window, "innerWidth", {
    configurable: true,
    value: width,
  });
  for (const listener of mediaListeners.get("(min-width: 768px)") ?? []) {
    listener({
      matches: matches("(min-width: 768px)"),
      media: "(min-width: 768px)",
    } as MediaQueryListEvent);
  }
}

function setSystemDark(dark: boolean): void {
  systemDark = dark;
  for (const listener of mediaListeners.get("(prefers-color-scheme: dark)") ??
    []) {
    listener({
      matches: dark,
      media: "(prefers-color-scheme: dark)",
    } as MediaQueryListEvent);
  }
}

function settingsStore(): Map<string, unknown> {
  let store = persistence.stores.get("settings.json");
  if (!store) {
    store = new Map();
    persistence.stores.set("settings.json", store);
  }
  return store;
}

function installDeviceLibrary(): void {
  nativeDeviceConfigured = true;
}

beforeEach(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
  installTauriRuntime();
  persistence.stores.clear();
  nativeDeviceConfigured = false;
  nativeProviderConnected = false;
  pendingImports = [];
  rejectDiscard = false;
  pinAcknowledgement=null;
  readFailure = null;
  positionAcknowledgement = null;
  extraDocument = false;
  deviceDocument.pinned = false;
  deviceDocument.name = "architecture.md";
  deviceDocument.revision = "a".repeat(64);
  manifestAcknowledgement = null;
  deviceMarkdown = "# Semantic architecture\n\nProvenance-aware reading.";
  nativeCalls.mockClear();
  mediaListeners.clear();
  viewportWidth = 390;
  systemDark = false;
  installMatchMedia();
  setViewport(390);
  setTestPlatform("ios");
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
  useSettingsStore.getState().reset();
  setTestPlatform("macos");
});

function openReaderOptions(panel = 'Actions') {
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  if (panel !== 'Actions') fireEvent.click(screen.getByRole('tab', { name: panel }));
}
function closeReaderOptions() {
  const close = screen.queryByRole('button', { name: 'Close reader options' });
  if (close) fireEvent.click(close);
}

describe("compile-time mobile composition", () => {
  test("device reader appearance keeps its mounted copy and persists shared font/size settings", async () => {
    installDeviceLibrary();
    const view = render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "architecture.md" }));
    await screen.findByRole("heading", { name: /Semantic architecture/ });
    const body = view.container.querySelector('.mdreader');
    const reads = nativeCalls.mock.calls.filter(([command]) => command === 'mobile_reader_read').length;
    openReaderOptions('Appearance');
    fireEvent.click(screen.getByRole('radio', { name: 'Sans serif' }));
    fireEvent.click(screen.getByRole('button', { name: 'Larger text' }));
    expect(view.container.querySelector('.mdreader')).toBe(body);
    const document = view.container.querySelector<HTMLElement>('.mobile-reader__document')!;
    expect(document.style.getPropertyValue('--font-reading-serif')).toBe('var(--font-reading-sans)');
    expect(document.style.getPropertyValue('--text-0')).toBe('calc(1rem * 1.08)');
    expect(nativeCalls.mock.calls.filter(([command]) => command === 'mobile_reader_read')).toHaveLength(reads);
    await waitFor(() => expect(settingsStore().get('settings')).toEqual(expect.objectContaining({ readingFont: 'sans', textScaleIndex: 4 })));
    closeReaderOptions();
    fireEvent.click(screen.getByRole('button', { name: 'Back' }));
    await screen.findByRole('navigation', { name: 'Primary' });
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    expect((screen.getByRole('radio', { name: 'Sans serif' }) as HTMLInputElement).checked).toBe(true);
    expect(screen.getByLabelText('Text size').textContent).toBe('108%');
  });

  test("a pasted cloud reading link opens its cached native revision and returns to Library", async () => {
    installDeviceLibrary(); nativeProviderConnected = true;
    render(<App />);
    const entry = await screen.findByLabelText('MDReader reading link');
    fireEvent.click(screen.getByText('Open reading link', { selector: 'summary' }));
    fireEvent.change(entry, { target: { value: readingLink({ sourceId: providerProjection.readerSourceId,
      path: providerProjection.path, revision: providerProjection.revision, line: 3 }) } });
    fireEvent.click(screen.getByRole('button', { name: 'Open document' }));
    await screen.findByRole('heading', { name: /Cached cloud note/ });
    expect(nativeCalls.mock.calls.some(([command, args]) => command === 'native_provider_request' &&
      ['fetch_current', 'connect'].includes((args as { request: { operation: string } }).request.operation))).toBe(false);
    expect(screen.queryByRole('navigation', { name: 'Primary' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Back to library' }));
    await screen.findByRole('heading', { name: 'Your Markdown' });
  });

  test("opens a selected wiki filename and its heading without inferring folders", async () => {
    installDeviceLibrary(); extraDocument = true;
    deviceMarkdown = "# Original\n\n[[second#Section two|Next section]]";
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "architecture.md" }));
    fireEvent.click(await screen.findByRole("link", { name: "Next section" }));
    await screen.findByRole("heading", { name: /Second selection/ });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(nativeCalls).toHaveBeenCalledWith("mobile_reader_read", { id: secondDocument.id, revision: secondDocument.revision });
  });
  test("a late position acknowledgment cannot close a newer tablet selection", async () => {
    installDeviceLibrary(); extraDocument = true; setViewport(900);
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { frames.push(callback); return frames.length; });
    vi.stubGlobal("cancelAnimationFrame", vi.fn());
    const view = render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "architecture.md" }));
    await screen.findByRole("heading", { name: /Semantic architecture/ });
    act(() => {
      fireEvent.scroll(view.container.querySelector(".mdreader")!);
      frames.splice(0).forEach((callback) => callback(0));
    });
    let finish!: () => void;
    positionAcknowledgement = new Promise<void>((resolve) => { finish = resolve; });
    closeReaderOptions();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await waitFor(() => expect(nativeCalls.mock.calls.some(([command]) => command === "mobile_reader_position")).toBe(true));
    fireEvent.click(screen.getByRole("button", { name: "second.md" }));
    await screen.findByRole("heading", { name: /Second selection/ });
    await act(async () => finish());
    expect(screen.getByRole("heading", { name: /Second selection/ })).toBeTruthy();
    expect(screen.getByRole("main", { name: "Reader" })).toBeTruthy();
  });
  test("ends a failed read and retries the same revision only on request", async () => {
    installDeviceLibrary();
    readFailure = "Source authority could not be verified";
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "architecture.md" }));
    expect((await screen.findByRole("alert")).textContent).toContain(readFailure);
    expect(screen.queryByText("Reading Markdown…")).toBeNull();
    const retry = screen.getByRole("button", { name: "Retry reading" });
    expect(nativeCalls.mock.calls.filter(([command]) => command === "mobile_reader_read")).toHaveLength(1);
    readFailure = null;
    fireEvent.click(retry);
    await screen.findByRole("heading", { name: /Semantic architecture/ });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(nativeCalls.mock.calls.filter(([command]) => command === "mobile_reader_read")).toHaveLength(2);
    expect(nativeCalls).toHaveBeenLastCalledWith("mobile_reader_read", {
      id: deviceDocument.id, revision: deviceDocument.revision,
    });
  });
  test("keeps interrupted imports visible until native removal succeeds", async () => {
    pendingImports = [{ id: deviceDocument.id, name: "partial.md", bytes: 128 }];
    rejectDiscard = true;
    render(<App />);
    const remove = await screen.findByRole("button", { name: "Remove incomplete copy of partial.md" });
    expect(screen.queryByRole("button", { name: "partial.md" })).toBeNull();
    fireEvent.click(remove);
    expect((await screen.findByRole("alert")).textContent).toContain("Incomplete copy could not be removed");
    expect(screen.getByRole("button", { name: "Remove incomplete copy of partial.md" })).toBeTruthy();
    rejectDiscard = false;
    fireEvent.click(remove);
    await waitFor(() => expect(screen.queryByLabelText("Interrupted imports")).toBeNull());
    expect(nativeCalls).toHaveBeenCalledWith("mobile_reader_discard_import", { id: deviceDocument.id });
  });
  test("imports original UTF-8 bytes and pins only after native acknowledgment", async () => {
    render(<App />);
    await screen.findByText(
      "No documents yet. Import Markdown to read offline.",
    );
    const content = "\uFEFF# Imported\r\n\r\nOffline text.\r\n";
    const file = new File([content], "notes.md", { type: "text/markdown" });
    Object.defineProperty(file, "arrayBuffer", {
      value: async () => new TextEncoder().encode(content).buffer,
    });
    fireEvent.change(screen.getByLabelText("Import Markdown"), {
      target: { files: [file] },
    });
    fireEvent.click(await screen.findByRole("button", { name: "notes.md" }));
    expect(
      await screen.findByText("Imported", { selector: "h1" }),
    ).toBeTruthy();
    expect(nativeCalls).toHaveBeenCalledWith("mobile_reader_import", {
      name: "notes.md",
      content,
    });
    openReaderOptions();
    fireEvent.click(screen.getByRole("button", { name: "Pin" }));
    await screen.findByRole("button", { name: "Unpin" });
    expect(screen.getByText("Imported", { selector: "h1" })).toBeTruthy();
    closeReaderOptions();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await screen.findByText(/Pinned ·/);
  });
  test("a delayed pin acknowledgement preserves Back navigation and the newly selected tab", async () => {
    installDeviceLibrary();
    let acknowledge!:()=>void;
    pinAcknowledgement=new Promise<void>(resolve=>{acknowledge=resolve;});
    render(<App />);
    fireEvent.click(await screen.findByRole("button",{name:"architecture.md"}));
    await screen.findByText("Semantic architecture",{selector:"h1"});
    openReaderOptions();
    fireEvent.click(screen.getByRole("button",{name:"Pin"}));
    await waitFor(()=>expect(nativeCalls).toHaveBeenCalledWith("mobile_reader_pin",{id:deviceDocument.id,pinned:true}));
    closeReaderOptions();
    fireEvent.click(screen.getByRole("button",{name:"Back"}));
    await screen.findByRole("navigation",{name:"Primary"});
    fireEvent.click(screen.getByRole("button",{name:"Search"}));
    await act(async()=>acknowledge());
    await waitFor(()=>expect(deviceDocument.pinned).toBe(true));
    expect(screen.queryByRole("main",{name:"Reader"})).toBeNull();
    expect(screen.getByRole("button",{name:"Search"}).getAttribute("aria-current")).toBe("page");
    fireEvent.click(screen.getByRole("button",{name:"Library"}));
    await screen.findByText(/Pinned ·/);
  });
  test.each(["ios", "android"] as const)(
    "%s mounts the phone tabs without mounting desktop chrome",
    (runtime) => {
      setTestPlatform(runtime);
      render(<App />);

      const navigation = screen.getByRole("navigation", { name: "Primary" });
      expect(navigation.textContent).toBe("LibrarySearchSourcesSettings");
      expect(screen.queryByText("No open documents")).toBeNull();
      expect(screen.queryByText("Source")).toBeNull();
      expect(screen.queryByText("Split")).toBeNull();
      expect(
        screen.queryByRole("button", { name: /check for updates/i }),
      ).toBeNull();
    },
  );

  test("Sources uses native provider availability without connecting or a direct credential form", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Sources" }));

    for (const source of [
      "Imported Markdown",
      "Google Drive",
      "Dropbox",
    ]) {
      expect(await screen.findByRole("heading", { name: source })).toBeTruthy();
    }
    for (const forbidden of [
      "Database",
      "Warehouse",
      "Host",
      "Port",
      "Username",
      "Password",
    ]) {
      expect(screen.queryByLabelText(forbidden)).toBeNull();
    }
    expect((screen.getByRole('button', { name: 'Connect Google Drive' }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: 'Connect Dropbox folder' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.queryByRole('checkbox', { name: 'Allow editing' })).toBeNull();
    expect(nativeCalls.mock.calls.filter(([command]) => command === 'native_provider_connect')).toHaveLength(0);
    expect(screen.queryByText("Enterprise CatDB server")).toBeNull();
    expect(screen.queryByRole("button", { name: "Add folder" })).toBeNull();
    expect(screen.queryByLabelText("Native connection")).toBeNull();
    expect(screen.queryByLabelText("Server URL")).toBeNull();
    expect(screen.queryByLabelText("Session reference")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("heading", { name: "Account & sync" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Account & sync" })).toBeTruthy();
  });

  test('Sources explicitly browses native metadata and opens the exact cached provider receipt without a device read or remote fetch', async () => {
    nativeProviderConnected = true;
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'Sources' }));
    await screen.findByRole('heading', { name: 'Selected Drive notes' });
    expect(nativeCalls.mock.calls.filter(([command, args]) => command === 'native_provider_request' && args?.request.operation === 'documents')).toHaveLength(0);
    fireEvent.click(screen.getByRole('button', { name: 'Browse documents' }));
    await screen.findByText('/Notes/cloud-note.md');
    fireEvent.click(screen.getByRole('button', { name: 'Read cached revision' }));
    await screen.findByRole('heading', { name: /Cached cloud note/ });
    const cloudReader = screen.getByRole('region', { name: 'Cloud document' });
    expect(within(cloudReader).getByText('Owned selected provider revision.')).toBeTruthy();
    expect(nativeCalls).toHaveBeenCalledWith('native_provider_request', { request: { operation: 'read_document', sourceId: providerProjection.readerSourceId, path: providerProjection.path, revision: providerProjection.revision } });
    expect(nativeCalls.mock.calls.filter(([command]) => command === 'mobile_reader_read')).toHaveLength(0);
    expect(nativeCalls.mock.calls.filter(([command, args]) => command === 'native_provider_request' && args?.request.operation === 'fetch_current')).toHaveLength(0);
    openReaderOptions();
    fireEvent.click(await screen.findByRole('button', { name: 'Export Markdown' }));
    await screen.findByText('Markdown exported to your selected destination.');
    expect(nativeCalls).toHaveBeenCalledWith('mobile_provider_transfer', { reference: { sourceId: providerProjection.readerSourceId, path: providerProjection.path, revision: providerProjection.revision }, action: 'export' });
    expect(screen.queryByRole('button', { name: 'Edit cloud document' })).toBeNull();
    closeReaderOptions();
    fireEvent.click(screen.getByRole('button', { name: 'Back to sources' }));
    expect(screen.getByText('/Notes/cloud-note.md')).toBeTruthy();
  });

  test("reader and Sources open the same account settings without uploading the selected copy", async () => {
    installDeviceLibrary(); render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "architecture.md" }));
    await screen.findByRole("heading", { name: /Semantic architecture/ });
    openReaderOptions();
    fireEvent.click(screen.getByRole("button", { name: "Sync this document" }));
    const heading = await screen.findByRole("heading", { name: "Account & sync" });
    await waitFor(() => expect(document.activeElement).toBe(heading));
    expect(screen.getByText("architecture.md", { selector: ".mobile-sync-selection strong" })).toBeTruthy();
    expect(nativeCalls.mock.calls.filter(([command]) => command === "reader_account")
      .every(([, args]) => (args as { operation: { operation: string } }).operation.operation === "status")).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Sources" }));
    fireEvent.click(screen.getByRole("button", { name: "View sync settings" }));
    await screen.findByRole("heading", { name: "Account & sync" });
  });

  test("an applied native revision change invalidates the reading snapshot and upload selection without opening newer content", async () => {
    installDeviceLibrary(); render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "architecture.md" }));
    await screen.findByRole("heading", { name: /Semantic architecture/ });
    const assetRequests = nativeCalls.mock.calls.filter(([command]) => command === "mobile_reader_assets").length;
    deviceDocument.revision = "d".repeat(64);
    act(() => window.dispatchEvent(new CustomEvent(DEVICE_READ_CHANGED)));
    await screen.findByText("This exact reading copy changed or was removed. Return to Library to choose an available copy.");
    expect(screen.queryByRole("heading", { name: /Semantic architecture/ })).toBeNull();
    expect(nativeCalls.mock.calls.filter(([command]) => command === "mobile_reader_read")).toHaveLength(1);
    expect(nativeCalls.mock.calls.filter(([command]) => command === "mobile_reader_assets")).toHaveLength(assetRequests);
    expect(screen.queryByText("Offline images", { selector: "summary" })).toBeNull();
    openReaderOptions();
    expect(screen.getByRole("tab", { name: "Details" })).toBeTruthy();
    expect((screen.getByRole("button", { name: "Retry reading", hidden: true }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Sync this document" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Pin" }) as HTMLButtonElement).disabled).toBe(true);
    closeReaderOptions();
    fireEvent.click(screen.getByRole("button", { name: "Back" }));
    await screen.findByRole("navigation", { name: "Primary" });
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await screen.findByRole("heading", { name: "Account & sync" });
    expect(screen.queryByText("architecture.md", { selector: ".mobile-sync-selection strong" })).toBeNull();
  });

  test("a late asset manifest cannot revive a retired reading revision", async () => {
    installDeviceLibrary();
    let complete!: () => void;
    manifestAcknowledgement = new Promise<void>((resolve) => { complete = resolve; });
    render(<App />);
    fireEvent.click(await screen.findByRole("button", { name: "architecture.md" }));
    await screen.findByRole("heading", { name: /Semantic architecture/ });
    deviceDocument.revision = "d".repeat(64);
    act(() => window.dispatchEvent(new CustomEvent(DEVICE_READ_CHANGED)));
    await screen.findByText("This exact reading copy changed or was removed. Return to Library to choose an available copy.");
    await act(async () => complete());
    expect(nativeCalls.mock.calls.filter(([command]) => command === "mobile_reader_assets")).toHaveLength(1);
    expect(screen.queryByText("Offline images", { selector: "summary" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Reload selected images" })).toBeNull();
  });

  test("hydrates and persists System, Light, and Dark choices across mobile relaunch", async () => {
    settingsStore().set("settings", {
      theme: "dark",
      readingFont: "serif",
      textScaleIndex: 3,
      sidebarCollapsed: false,
      outlineVisible: true,
      relatedRailOpen: false,
      inspectorOpen: false,
      lensSchema: null,
      reviewer: "",
    });
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() =>
      expect(
        (screen.getByRole("radio", { name: "Dark" }) as HTMLInputElement)
          .checked,
      ).toBe(true),
    );
    expect(document.documentElement.dataset.theme).toBe("dark");

    fireEvent.click(screen.getByRole("radio", { name: "Light" }));
    await waitFor(() =>
      expect(settingsStore().get("settings")).toEqual(
        expect.objectContaining({ theme: "light" }),
      ),
    );

    cleanup();
    useSettingsStore.getState().reset();
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() =>
      expect(
        (screen.getByRole("radio", { name: "Light" }) as HTMLInputElement)
          .checked,
      ).toBe(true),
    );
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(screen.getByRole("radio", { name: "System" })).toBeTruthy();
  });

  test("System follows a dark OS without converting the persisted choice to explicit Dark", async () => {
    setSystemDark(true);
    render(<App />);

    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    await waitFor(() =>
      expect(
        (screen.getByRole("radio", { name: "System" }) as HTMLInputElement)
          .checked,
      ).toBe(true),
    );
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
    expect(
      document
        .querySelector(".mobile-shell")
        ?.getAttribute("data-system-theme"),
    ).toBe("dark");
  });

  test("hydrates SQLite imports into Library, full-text search, Reader, and revision evidence", async () => {
    installDeviceLibrary();
    render(<App />);

    expect(
      await screen.findByRole("button", { name: "architecture.md" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Search" }));
    fireEvent.change(screen.getByRole("searchbox", { name: "Search" }), {
      target: { value: "Provenance-aware" },
    });
    fireEvent.click(
      await screen.findByRole("button", { name: "architecture.md" }),
    );
    expect(nativeCalls).toHaveBeenCalledWith("mobile_search_query", {
      query: "Provenance-aware",
      mode: "text",
    });
    expect(
      await screen.findByText("Semantic architecture", { selector: "h1" }),
    ).toBeTruthy();
    expect(screen.getByRole("main", { name: "Reader" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Library" })).toBeNull();

    openReaderOptions("Details");
    expect(screen.getByRole("dialog", { name: "Reader options" })).toBeTruthy();
    expect(screen.getByRole("tab", { name: "Details" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText(deviceDocument.revision)).toBeTruthy();
    expect(screen.getByText(deviceDocument.source_id)).toBeTruthy();
    expect(nativeCalls).toHaveBeenCalledWith("mobile_reader_read", {
      id: deviceDocument.id,
      revision: deviceDocument.revision,
    });
    expect(screen.queryByRole("navigation", { name: "Primary" })).toBeNull();
  });

  test("reacts to phone/tablet viewport changes in both directions while reading", async () => {
    installDeviceLibrary();
    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "architecture.md" }),
    );
    expect(await screen.findByRole("main", { name: "Reader" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Library" })).toBeNull();

    setViewport(900);
    await waitFor(() =>
      expect(screen.getByRole("region", { name: "Library" })).toBeTruthy(),
    );

    setViewport(390);
    await waitFor(() =>
      expect(screen.queryByRole("region", { name: "Library" })).toBeNull(),
    );
  });
});
