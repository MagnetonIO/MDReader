import { useEffect, useId, useState } from "react";
import { readerBackend } from "../platform/backend";
import "../features/settings/indexing.css";
type Policy = "balanced" | "paused" | "full_speed";
const descriptions: Record<Policy, string> = {
  balanced: "Balances indexing speed with a responsive reader.",
  paused: "Pauses background indexing after the current batch finishes.",
  full_speed: "Finishes indexing faster with higher CPU use.",
};
interface Backend {
  get(): Promise<string>;
  set(policy: Policy): Promise<string>;
}
const backend: Backend = {
  async get() {
    const value = await readerBackend().search.readerEmbeddingPolicy();
    if (value.status === "error") throw new Error(value.error);
    return value.data;
  },
  async set(policy) {
    const value = await readerBackend().search.setReaderEmbeddingPolicy(policy);
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
  const descriptionId = useId();
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
      className="settings-section indexing-settings"
      aria-label="Background semantic indexing"
    >
      <h3>Background semantic indexing</h3>
      <p>
        Choose how MDReader builds its search index in the background.
        Reading and text search stay available in every mode.
      </p>
      <div className="indexing-settings__controls">
        <label className="indexing-settings__field">
          <span>Indexing mode</span>
          <span className="indexing-settings__select">
            <select
              aria-label="Indexing mode"
              aria-describedby={descriptionId}
              value={selected}
              disabled={busy || applied === null}
              onChange={(event) => setSelected(event.currentTarget.value as Policy)}
            >
              <option value="balanced">Balanced</option>
              <option value="paused">Paused</option>
              <option value="full_speed">Full speed</option>
            </select>
          </span>
        </label>
        <button
          className="indexing-settings__apply"
          type="button"
          disabled={busy || applied === null || selected === applied}
          onClick={() => void apply()}
        >
          {busy ? "Saving…" : "Apply indexing mode"}
        </button>
      </div>
      <p id={descriptionId} className="indexing-settings__hint">{descriptions[selected]}</p>
      <p className="indexing-settings__status" role="status">
        {applied === null
          ? "Reading indexing policy…"
          : `Current mode: ${applied === "full_speed" ? "Full speed" : applied === "paused" ? "Paused" : "Balanced"}`}
      </p>
      {error ? <p role="alert">{error}</p> : null}
    </section>
  );
}
