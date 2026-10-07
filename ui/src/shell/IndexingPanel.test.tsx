import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { IndexingPanel } from "./IndexingPanel";
afterEach(cleanup);
test("explicit indexing choice reports only the acknowledged mode", async () => {
  let resolve!: (value: string) => void;
  const set = vi.fn(
    () =>
      new Promise<string>((done) => {
        resolve = done;
      }),
  );
  render(<IndexingPanel service={{ get: async () => "balanced", set }} />);
  await screen.findByText("Current mode: Balanced");
  fireEvent.change(screen.getByLabelText("Indexing mode"), {
    target: { value: "paused" },
  });
  expect(set).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Apply indexing mode" }));
  expect(set).toHaveBeenCalledWith("paused");
  expect(screen.getByText("Current mode: Balanced")).toBeTruthy();
  resolve("paused");
  await screen.findByText("Current mode: Paused");
});
test("failed policy persistence preserves the last applied mode", async () => {
  render(
    <IndexingPanel
      service={{
        get: async () => "paused",
        set: async () => {
          throw new Error("Disk full");
        },
      }}
    />,
  );
  await screen.findByText("Current mode: Paused");
  fireEvent.change(screen.getByLabelText("Indexing mode"), {
    target: { value: "full_speed" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Apply indexing mode" }));
  await waitFor(() =>
    expect(screen.getByRole("alert").textContent).toBe("Disk full"),
  );
  expect(screen.getByText("Current mode: Paused")).toBeTruthy();
});
