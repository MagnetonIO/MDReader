// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { DeviceSearch } from "./DeviceSearch";
import type { DeviceSearchSession } from "./DeviceSearch";

const backend = vi.hoisted(() => ({
  mobileSearchSettings: vi.fn(async () => ({ status: "ok", data: { enabled: true, model: "ready", documents: 2, embedded_documents: 2 } })),
  mobileSearchQuery: vi.fn(async () => ({ status: "ok", data: { hits: [], semantic_unavailable: null } })),
  mobileSearchSetEnabled: vi.fn(),
}));
vi.mock("../bindings", () => ({ commands: backend }));
afterEach(cleanup);

test("returning from a passage preserves the query and mode and retrieves fresh results", async () => {
  const session: DeviceSearchSession = { query: "", mode: "text" };
  const first = render(<DeviceSearch session={session} onOpen={() => {}} />);
  await screen.findByText("Ready · 2 of 2 documents indexed");
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
