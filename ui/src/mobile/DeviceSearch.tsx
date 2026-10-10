import { useEffect, useRef, useState } from "react";
import { commands } from "../bindings";
import type { DeviceDocument, DeviceHit, DeviceSearchMode, DeviceSearchSettings } from "../bindings";
import { ChevronRight } from "../shell/icons";

async function result<T>(request: Promise<{ status: "ok"; data: T } | { status: "error"; error: string }>): Promise<T> {
  const reply = await request;
  if (reply.status === "error") throw new Error(reply.error);
  return reply.data;
}
const message = (error: unknown) => error instanceof Error ? error.message : "Search could not finish.";
export interface DeviceSearchSession { query: string; mode: DeviceSearchMode }

/** Search owns its request epochs and model consent; reading owns navigation. */
export function DeviceSearch({ onOpen, session }: { onOpen: (document: DeviceDocument, line: number) => void; session: DeviceSearchSession }) {
  const [query, setQuery] = useState(session.query);
  const [mode, setMode] = useState<DeviceSearchMode>(session.mode);
  const changeQuery = (value: string) => { session.query = value; setQuery(value); };
  const changeMode = (value: DeviceSearchMode) => { session.mode = value; setMode(value); };
  const [settings, setSettings] = useState<DeviceSearchSettings | null>(null);
  const [hits, setHits] = useState<DeviceHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [changing, setChanging] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [diagnostic, setDiagnostic] = useState<string | null>(null);
  const [meaningOpen, setMeaningOpen] = useState(false);
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
          if (value.indexed_documents === value.documents && value.model === "ready" && value.embedded_documents === value.documents) delay = 15000;
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
  const searchIdentity = JSON.stringify([query, selectedMode, settings?.lexical_snapshot, settings?.model, settings?.embedded_documents]);
  const currentSearchIdentity = useRef(searchIdentity);
  // Fence replies as soon as render sees new readiness, before effect cleanup.
  currentSearchIdentity.current = searchIdentity;
  useEffect(() => {
    const request = ++epoch.current;
    const current = () => request === epoch.current && searchIdentity === currentSearchIdentity.current;
    setHits([]); setError(null); setDiagnostic(null);
    if (!query.trim()) { setSearching(false); return; }
    setSearching(true);
    const timer = setTimeout(() => {
      void result(commands.mobileSearchQuery(query, selectedMode)).then((value) => {
        if (current()) { setHits(value.hits); setDiagnostic(value.semantic_unavailable); }
      }).catch((cause: unknown) => {
        if (current()) setError(message(cause));
      }).finally(() => { if (current()) setSearching(false); });
    }, 200);
    return () => { clearTimeout(timer); epoch.current++; };
  }, [query, selectedMode, searchIdentity]);

  const changeConsent = async () => {
    const request = ++consentEpoch.current;
    mutatingConsent.current = true;
    setChanging(true); setStatusError(null);
    try {
      const value = await result(commands.mobileSearchSetEnabled(!enabled));
      if (request === consentEpoch.current) {
        setSettings(value);
        changeMode(value.enabled ? "hybrid" : "text");
      }
    } catch (cause) { if (request === consentEpoch.current) { setStatusError(message(cause)); setMeaningOpen(true); } }
    finally { mutatingConsent.current = false; if (request === consentEpoch.current) setChanging(false); }
  };
  const modelLabel = !settings ? "Checking…" : !enabled ? "Off" : settings?.model === "loading" ? "Preparing…"
    : settings?.model === "ready" ? "Ready" : settings?.model === "not_downloaded" ? "Preparing…"
      : "Unavailable";
  return <section className="mobile-screen" aria-label="Search">
    <h1>Search</h1>
    <label className="mobile-field">Search
      <input type="search" placeholder={selectedMode === "text" ? "Words in your documents" : "Describe what you’re looking for"}
        value={query} onChange={(event) => changeQuery(event.currentTarget.value)} maxLength={1024} />
    </label>
    <label className="mobile-field">Search mode
      <select value={selectedMode} onChange={(event) => changeMode(event.currentTarget.value as DeviceSearchMode)}>
        <option value="text">Text</option><option value="hybrid" disabled={!enabled}>Text + meaning</option>
        <option value="semantic" disabled={!enabled}>Meaning</option>
      </select>
    </label>
    <p>{selectedMode === "text" ? "Find words and phrases in your documents." : "Find related ideas using the model on this device."}</p>
    {settings && settings.indexed_documents < settings.documents ? <p role="status">
      Text search covers {settings.indexed_documents} of {settings.documents} documents. Some documents are not searchable yet.
    </p> : null}
    {error ? <p role="alert">{error}</p> : diagnostic ? <p role="status">{diagnostic}</p> : null}
    {searching ? <p role="status">Searching…</p> : query && !hits.length && !error && !diagnostic ? <p>No matching passages.</p> : null}
    <ol className="mobile-document-list">{hits.map((hit) => <li key={`${hit.document.id}:${hit.line}`}>
      <button type="button" className="mobile-document mobile-search-result" aria-label={hit.document.name} onClick={() => onOpen(hit.document, hit.line)}>
        <span className="mobile-search-result__body"><strong>{hit.document.name}</strong>
          <small>Line {hit.line}</small><span className="mobile-search-result__snippet">{hit.text}</span>
        </span>
      </button>
    </li>)}</ol>
    <details className="mobile-source mobile-search-model" open={meaningOpen} onToggle={(event) => setMeaningOpen(event.currentTarget.open)}>
      <summary><ChevronRight className="mobile-search-model__chevron" aria-hidden="true" />
        <span>Search by meaning</span><span className="mobile-search-model__status" role="status">{modelLabel}</span>
      </summary>
      {meaningOpen ? <div className="mobile-search-model__content">
        <p>Downloads an English search model from Hugging Face. Your documents stay on this device. After setup, search works offline.</p>
        {enabled && settings ? <p role="status">{settings.embedded_documents} of {settings.documents} documents ready for meaning search</p> : null}
        {modelLabel === "Unavailable" ? <p>Restart the app to retry model setup.</p> : null}
        <button type="button" disabled={!settings || changing} onClick={() => void changeConsent()}>
          {changing ? "Updating…" : enabled ? "Turn off semantic search" : "Download & enable"}
        </button>
        {enabled ? <p>Turning this off stops new model work and keeps the downloaded model. A running operation may finish.</p> : null}
      </div> : null}
    </details>
    {statusError ? <p role="alert">{statusError}</p> : null}
  </section>;
}
