import { useEffect, useState } from "react";
import { commands } from "../bindings";
type Policy = "balanced" | "paused" | "full_speed";
interface Backend {
  get(): Promise<string>;
  set(policy: Policy): Promise<string>;
}
const backend: Backend = {
  async get() {
    const value = await commands.readerEmbeddingPolicy();
    if (value.status === "error") throw new Error(value.error);
    return value.data;
  },
  async set(policy) {
    const value = await commands.setReaderEmbeddingPolicy(policy);
    if (value.status === "error") throw new Error(value.error);
    return value.data;
  },
};
function policy(value: string): Policy {
  if (value === "balanced" || value === "paused" || value === "full_speed")
    return value;
  throw new Error("Unknown indexing policy; restart the reader service.");
}
export function IndexingPanel({ service = backend }: { service?: Backend }) {
  const [selected, setSelected] = useState<Policy>("balanced");
  const [applied, setApplied] = useState<Policy | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    void service
      .get()
      .then((value) => {
        if (current) {
          const checked = policy(value);
          setApplied(checked);
          setSelected(checked);
        }
      })
      .catch((cause) => {
        if (current)
          setError(
            cause instanceof Error
              ? cause.message
              : "Indexing status unavailable",
          );
      });
    return () => {
      current = false;
    };
  }, [service]);
  const apply = async () => {
    setBusy(true);
    setError(null);
    try {
      setApplied(policy(await service.set(selected)));
    } catch (cause) {
      setError(
        cause instanceof Error
          ? cause.message
          : "Indexing policy was not saved",
      );
    } finally {
      setBusy(false);
    }
  };
  return (
    <section
      className="agent-capture-panel"
      aria-label="Background semantic indexing"
    >
      <h3>Background semantic indexing</h3>
      <p>
        Balanced limits inference to one CPU thread and adds time between
        batches. Reading and text search remain available. A running batch
        finishes before a pause takes effect.
      </p>
      <label>
        Indexing mode
        <select
          aria-label="Indexing mode"
          value={selected}
          disabled={busy || applied === null}
          onChange={(event) => setSelected(event.currentTarget.value as Policy)}
        >
          <option value="balanced">Balanced</option>
          <option value="paused">Paused</option>
          <option value="full_speed">Full speed</option>
        </select>
      </label>
      <button
        type="button"
        disabled={busy || applied === null || selected === applied}
        onClick={() => void apply()}
      >
        {busy ? "Saving…" : "Apply indexing mode"}
      </button>
      <p role="status">
        {applied === null
          ? "Reading indexing policy…"
          : `Current mode: ${applied === "full_speed" ? "Full speed" : applied === "paused" ? "Paused" : "Balanced"}`}
      </p>
      {error ? <p role="alert">{error}</p> : null}
    </section>
  );
}
