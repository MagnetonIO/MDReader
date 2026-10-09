import { useEffect, useRef, useState } from "react";
import { commands } from "../bindings";
import type { DeviceDocument, DeviceHit, DeviceSearchMode, DeviceSearchSettings } from "../bindings";

async function result<T>(request: Promise<{ status: "ok"; data: T } | { status: "error"; error: string }>): Promise<T> {
  const reply = await request;
  if (reply.status === "error") throw new Error(reply.error);
  return reply.data;
}
const message = (error: unknown) => error instanceof Error ? error.message : "Search could not finish.";

/** Search owns its request epochs and model consent; reading owns navigation. */
export function DeviceSearch({ onOpen }: { onOpen: (document: DeviceDocument, line: number) => void }) {
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<DeviceSearchMode>("text");
  const [settings, setSettings] = useState<DeviceSearchSettings | null>(null);
  const [hits, setHits] = useState<DeviceHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [diagnostic, setDiagnostic] = useState<string | null>(null);
  const epoch = useRef(0);
  const consentEpoch = useRef(0);
  const mutatingConsent = useRef(false);
  useEffect(() => {
    let current = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      let delay = 3000;
      const consent = consentEpoch.current;
      if (!document.hidden && !mutatingConsent.current) {
        try {
          const value = await result(commands.mobileSearchSettings());
          if (value.model === "ready" && value.embedded_documents === value.documents) delay = 15000;
          if (current && consent === consentEpoch.current) { setSettings(value); setStatusError(null); }
        } catch (cause) { if (current) setStatusError(message(cause)); }
      }
      if (current) timer = setTimeout(() => void poll(), delay);
    };
    void poll();
    return () => { current = false; clearTimeout(timer); consentEpoch.current++; };
  }, []);
  const enabled = settings?.enabled ?? false;
  const selectedMode = enabled ? mode : "text";
  useEffect(() => {
    const request = ++epoch.current;
    setHits([]); setError(null); setDiagnostic(null);
    if (!query.trim()) { setSearching(false); return; }
    setSearching(true);
    const timer = setTimeout(() => {
      void result(commands.mobileSearchQuery(query, selectedMode)).then((value) => {
        if (request === epoch.current) { setHits(value.hits); setDiagnostic(value.semantic_unavailable); }
      }).catch((cause: unknown) => {
        if (request === epoch.current) setError(message(cause));
      }).finally(() => { if (request === epoch.current) setSearching(false); });
    }, 200);
    return () => { clearTimeout(timer); epoch.current++; };
  }, [query, selectedMode, settings?.model, settings?.embedded_documents]);

  const changeConsent = async () => {
    const request = ++consentEpoch.current;
    mutatingConsent.current = true;
    setChanging(true); setStatusError(null);
    try {
      const value = await result(commands.mobileSearchSetEnabled(!enabled));
      if (request === consentEpoch.current) {
        setSettings(value);
        setMode(value.enabled ? "hybrid" : "text");
      }
    } catch (cause) { if (request === consentEpoch.current) setStatusError(message(cause)); }
    finally { mutatingConsent.current = false; if (request === consentEpoch.current) setChanging(false); }
  };
  const modelLabel = !enabled ? "Off" : settings?.model === "loading" ? "Downloading or preparing model…"
    : settings?.model === "ready" ? "Ready" : settings?.model === "not_downloaded" ? "Preparing model…"
      : "Unavailable — restart the app to retry";
  return <section className="mobile-screen" aria-label="Search">
    <h1>Search your Markdown</h1>
    <label className="mobile-field">Search
      <input type="search" placeholder={selectedMode === "text" ? "Words in your documents" : "Describe what you’re looking for"}
        value={query} onChange={(event) => setQuery(event.currentTarget.value)} maxLength={1024} />
    </label>
    <label className="mobile-field">Search mode
      <select value={selectedMode} onChange={(event) => setMode(event.currentTarget.value as DeviceSearchMode)}>
        <option value="text">Text</option><option value="hybrid" disabled={!enabled}>Text + meaning</option>
        <option value="semantic" disabled={!enabled}>Meaning</option>
      </select>
    </label>
    <p>{selectedMode === "text" ? "Find words and phrases in your documents." : "Find related ideas using the model on this device."}</p>
    {error ? <p role="alert">{error}</p> : diagnostic ? <p role="status">{diagnostic}</p> : null}
    {searching ? <p role="status">Searching…</p> : query && !hits.length && !error && !diagnostic ? <p>No matching passages.</p> : null}
    <ol className="mobile-document-list">{hits.map((hit) => <li key={`${hit.document.id}:${hit.line}`}>
      <button type="button" className="mobile-document" aria-label={hit.document.name} onClick={() => onOpen(hit.document, hit.line)}>
        <span><strong>{hit.document.name} · Line {hit.line}</strong><small>{hit.text}</small></span>
      </button>
    </li>)}</ol>
    <article className="mobile-source mobile-search-model" aria-label="Semantic search">
      <h2>Search by meaning</h2>
      <p>Downloads an English search model from Hugging Face. Your documents stay on this device. After setup, search works offline.</p>
      <p role="status">{modelLabel}{enabled && settings ? ` · ${settings.embedded_documents} of ${settings.documents} documents indexed` : ""}</p>
      <button type="button" disabled={!settings || changing} onClick={() => void changeConsent()}>
        {changing ? "Updating…" : enabled ? "Turn off semantic search" : "Download & enable"}
      </button>
      {enabled ? <p>Turning this off stops new model work and keeps the downloaded model. A running operation may finish.</p> : null}
      {statusError ? <p role="alert">{statusError}</p> : null}
    </article>
  </section>;
}
