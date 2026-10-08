import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import App from "./App";
import { useSettingsStore } from "./stores/settings";
import { setTestPlatform } from "./test/tauri-os";

const persistence = vi.hoisted(() => ({
  stores: new Map<string, Map<string, unknown>>(),
}));
let nativeDeviceConfigured = false;
let pendingImports: { id: string; name: string; bytes: number }[] = [];
let rejectDiscard = false;
let pinAcknowledgement:Promise<void>|null=null;
let readFailure: string | null = null;
let positionAcknowledgement: Promise<void> | null = null;
let extraDocument = false;
let deviceMarkdown = "# Semantic architecture\n\nProvenance-aware reading.";
const deviceDocument = {
  id: "11111111-1111-4111-8111-111111111111",
  name: "architecture.md",
  source_id: "device-source-1",
  revision: "a".repeat(64),
  bytes: 128,
  pinned: false,
  line: 1,
  indexed: true,
};
const secondDocument = { ...deviceDocument, id: "33333333-3333-4333-8333-333333333333", name: "second.md", source_id: "device-source-2" };
const nativeCalls = vi.fn();

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
        if (command === "mobile_reader_assets") {
          const request = args?.request as { operation: string; document_id?: string; revision?: string };
          if (request.operation === "manifest") return { operation: "manifest", value: {
            document_id: request.document_id, document_revision: request.revision,
            manifest: "b".repeat(64), assets: [], pending: [], targets: [], targets_truncated: false,
          } };
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
        if (command === "mobile_reader_search")
          return [
            {
              document: { ...deviceDocument },
              line: 3,
              text: "Provenance-aware reading.",
            },
          ];
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
  pendingImports = [];
  rejectDiscard = false;
  pinAcknowledgement=null;
  readFailure = null;
  positionAcknowledgement = null;
  extraDocument = false;
  deviceDocument.pinned = false;
  deviceDocument.name = "architecture.md";
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

describe("compile-time mobile composition", () => {
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
    fireEvent.click(screen.getByRole("button", { name: "Pin" }));
    await screen.findByRole("button", { name: "Unpin" });
    expect(screen.getByText("Imported", { selector: "h1" })).toBeTruthy();
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
    fireEvent.click(screen.getByRole("button",{name:"Pin"}));
    await waitFor(()=>expect(nativeCalls).toHaveBeenCalledWith("mobile_reader_pin",{id:deviceDocument.id,pinned:true}));
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

  test("Sources exposes only the mobile allowlist and no direct credential form", () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Sources" }));

    for (const source of [
      "Imported Markdown",
      "Google Drive",
      "Dropbox",
      "Encrypted cloud sync",
    ]) {
      expect(screen.getByRole("heading", { name: source })).toBeTruthy();
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
    expect(
      screen.getAllByText(
        "Unavailable until mobile secure sign-in and transfer are connected.",
      ),
    ).toHaveLength(3);
    expect(screen.queryByText("Enterprise CatDB server")).toBeNull();
    expect(screen.queryByRole("button", { name: "Add folder" })).toBeNull();
    expect(screen.queryByLabelText("Native connection")).toBeNull();
    expect(screen.queryByLabelText("Server URL")).toBeNull();
    expect(screen.queryByLabelText("Session reference")).toBeNull();
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
    expect(nativeCalls).toHaveBeenCalledWith("mobile_reader_search", {
      query: "Provenance-aware",
    });
    expect(
      await screen.findByText("Semantic architecture", { selector: "h1" }),
    ).toBeTruthy();
    expect(screen.getByRole("main", { name: "Reader" })).toBeTruthy();
    expect(screen.queryByRole("region", { name: "Library" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Evidence" }));
    expect(
      screen.getByRole("main", { name: "Document evidence" }),
    ).toBeTruthy();
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
