// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import type { DeviceHit, DeviceSearchResults, DeviceSearchSettings } from "../bindings";
import { DeviceSearch } from "./DeviceSearch";
import type { DeviceSearchSession } from "./DeviceSearch";

type Reply<T> = { status: "ok"; data: T } | { status: "error"; error: string };
const backend = vi.hoisted(() => ({
  mobileSearchSettings: vi.fn<() => Promise<Reply<DeviceSearchSettings>>>(),
  mobileSearchQuery: vi.fn<(query: string, mode: string) => Promise<Reply<DeviceSearchResults>>>(),
  mobileSearchSetEnabled: vi.fn(),
}));
vi.mock("../bindings", () => ({ commands: backend }));
const ready: DeviceSearchSettings = { enabled: true, model: "ready", documents: 2, embedded_documents: 2, indexed_documents: 2, lexical_snapshot: "ready-two" };
const pending: DeviceSearchSettings = { enabled: false, model: "disabled", documents: 1, embedded_documents: 0, indexed_documents: 0, lexical_snapshot: "pending-one" };
const lexicalReady: DeviceSearchSettings = { ...pending, indexed_documents: 1, lexical_snapshot: "ready-one" };
const empty: Reply<DeviceSearchResults> = { status: "ok", data: { hits: [], semantic_unavailable: null } };
const hit = (id: string): DeviceHit => ({
  document: { id, source_id: "public-device", revision: `${id}-revision`, name: `${id}.md`, bytes: 48, indexed: true, pinned: false, line: 1 },
  line: 3, text: "Public river fixture.",
});
const withHit = (id: string): Reply<DeviceSearchResults> => ({ status: "ok", data: { hits: [hit(id)], semantic_unavailable: null } });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((accept) => { resolve = accept; });
  return { promise, resolve };
}
async function tick(ms: number) { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); }
beforeEach(() => {
  vi.resetAllMocks();
  backend.mobileSearchSettings.mockResolvedValue({ status: "ok", data: ready });
  backend.mobileSearchQuery.mockResolvedValue(empty);
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

test("returning from a passage preserves the query and mode and retrieves fresh results", async () => {
  const session: DeviceSearchSession = { query: "", mode: "text" };
  const first = render(<DeviceSearch session={session} onOpen={() => {}} />);
  await screen.findByText("Ready");
  fireEvent.click(screen.getByText("Search by meaning"));
  await screen.findByText("2 of 2 documents ready for meaning search");
  fireEvent.change(screen.getByRole("searchbox"), { target: { value: "preserve original writing" } });
  fireEvent.change(screen.getByRole("combobox"), { target: { value: "semantic" } });
  await waitFor(() => expect(backend.mobileSearchQuery).toHaveBeenCalledWith("preserve original writing", "semantic"));
  first.unmount();
  backend.mobileSearchQuery.mockClear();
  render(<DeviceSearch session={session} onOpen={() => {}} />);
  expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("preserve original writing");
  await waitFor(() => expect((screen.getByRole("combobox") as HTMLSelectElement).value).toBe("semantic"));
  await waitFor(() => expect(backend.mobileSearchQuery).toHaveBeenCalledWith("preserve original writing", "semantic"));
});

test("the same text query refreshes when imported content becomes indexed with semantic search off", async () => {
  vi.useFakeTimers();
  backend.mobileSearchSettings.mockResolvedValueOnce({ status: "ok", data: pending })
    .mockResolvedValue({ status: "ok", data: lexicalReady });
  backend.mobileSearchQuery.mockResolvedValueOnce(empty).mockResolvedValue(withHit("new-import"));
  render(<DeviceSearch session={{ query: "river", mode: "text" }} onOpen={() => {}} />);
  await tick(0);
  expect(screen.getByText(/Text search covers 0 of 1 documents/)).toBeTruthy();
  await tick(200);
  expect(backend.mobileSearchQuery).toHaveBeenCalledTimes(1);
  await tick(3000);
  await tick(200);
  expect(backend.mobileSearchQuery).toHaveBeenCalledTimes(2);
  expect(backend.mobileSearchQuery).toHaveBeenLastCalledWith("river", "text");
  expect((screen.getByRole("searchbox") as HTMLInputElement).value).toBe("river");
  expect(screen.getByRole("button", { name: "new-import.md" })).toBeTruthy();
  expect(screen.queryByText(/not searchable yet/)).toBeNull();
  expect(screen.getByText("Off")).toBeTruthy();
  // Polling the identical native snapshot must not repeatedly rerun the query.
  await tick(3000);
  await tick(200);
  expect(backend.mobileSearchQuery).toHaveBeenCalledTimes(2);
});

test("a superseded lexical query cannot replace ready results or end the newer search", async () => {
  vi.useFakeTimers();
  const old = deferred<Reply<DeviceSearchResults>>();
  const current = deferred<Reply<DeviceSearchResults>>();
  backend.mobileSearchSettings.mockResolvedValueOnce({ status: "ok", data: pending })
    .mockResolvedValue({ status: "ok", data: lexicalReady });
  backend.mobileSearchQuery.mockReturnValueOnce(old.promise).mockReturnValueOnce(current.promise);
  render(<DeviceSearch session={{ query: "river", mode: "text" }} onOpen={() => {}} />);
  await tick(0);
  await tick(200);
  await tick(3000);
  await tick(200);
  expect(backend.mobileSearchQuery).toHaveBeenCalledTimes(2);
  await act(async () => { old.resolve(withHit("stale-import")); });
  expect(screen.queryByRole("button", { name: "stale-import.md" })).toBeNull();
  expect(screen.getByText("Searching…")).toBeTruthy();
  await act(async () => { current.resolve(withHit("current-import")); });
  expect(screen.getByRole("button", { name: "current-import.md" })).toBeTruthy();
  expect(screen.queryByText("Searching…")).toBeNull();
});

test("leaving Search cancels readiness polling and ignores its outstanding query", async () => {
  vi.useFakeTimers();
  const old = deferred<Reply<DeviceSearchResults>>();
  backend.mobileSearchSettings.mockResolvedValue({ status: "ok", data: lexicalReady });
  backend.mobileSearchQuery.mockReturnValue(old.promise);
  const view = render(<DeviceSearch session={{ query: "river", mode: "text" }} onOpen={() => {}} />);
  await tick(0);
  await tick(200);
  view.unmount();
  await act(async () => { old.resolve(withHit("late-import")); });
  await tick(10000);
  expect(backend.mobileSearchSettings).toHaveBeenCalledTimes(1);
  expect(backend.mobileSearchQuery).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("button", { name: "late-import.md" })).toBeNull();
});

test("meaning setup stays compact until expanded and only the explicit download action records consent", async () => {
  backend.mobileSearchSettings.mockResolvedValue({ status: "ok", data: lexicalReady });
  backend.mobileSearchSetEnabled.mockResolvedValue({ status: "ok", data: { ...lexicalReady, enabled: true, model: "loading" } });
  const session: DeviceSearchSession = { query: "", mode: "text" };
  render(<DeviceSearch session={session} onOpen={() => {}} />);
  await screen.findByText("Off");
  expect(screen.queryByRole("button", { name: "Download & enable" })).toBeNull();
  fireEvent.click(screen.getByText("Search by meaning"));
  const download = await screen.findByRole("button", { name: "Download & enable" });
  expect(download).toBeTruthy();
  expect(screen.getByText(/Your documents stay on this device/)).toBeTruthy();
  expect(backend.mobileSearchSetEnabled).not.toHaveBeenCalled();
  fireEvent.click(download);
  await screen.findByText("Preparing…");
  expect(backend.mobileSearchSetEnabled).toHaveBeenCalledExactlyOnceWith(true);
  expect(session.mode).toBe("hybrid");
  fireEvent.click(screen.getByText("Search by meaning"));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Turn off semantic search" })).toBeNull());
  expect(backend.mobileSearchSetEnabled).toHaveBeenCalledTimes(1);
});

test("a failed consent operation reopens setup if collapsed and leaves text search enabled", async () => {
  const consent = deferred<Reply<DeviceSearchSettings>>();
  backend.mobileSearchSettings.mockResolvedValue({ status: "ok", data: lexicalReady });
  backend.mobileSearchSetEnabled.mockReturnValue(consent.promise);
  const session: DeviceSearchSession = { query: "", mode: "text" };
  render(<DeviceSearch session={session} onOpen={() => {}} />);
  await screen.findByText("Off");
  fireEvent.click(screen.getByText("Search by meaning"));
  fireEvent.click(await screen.findByRole("button", { name: "Download & enable" }));
  await screen.findByRole("button", { name: "Updating…" });
  fireEvent.click(screen.getByText("Search by meaning"));
  await waitFor(() => expect(screen.queryByRole("button", { name: "Updating…" })).toBeNull());
  await act(async () => { consent.resolve({ status: "error", error: "Public model setup unavailable." }); });
  expect(screen.getByRole("alert").textContent).toBe("Public model setup unavailable.");
  expect(screen.getByRole("button", { name: "Download & enable" })).toBeTruthy();
  expect(session.mode).toBe("text");
  expect((screen.getByRole("combobox") as HTMLSelectElement).value).toBe("text");
});
