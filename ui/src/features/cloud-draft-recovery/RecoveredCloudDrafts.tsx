import { useEffect, useRef, useState } from 'react';
import { isTauri } from '../../shell/persistence';
import { cloudRecoveryBackend, type RecoveredDraft, type RecoveredDraftBody, type RecoveryBackend } from './backend';
import { RecoveredDraftDialog } from './RecoveredDraftDialog';

/** Local journal recovery never seeds a cloud editor or resurrects a save ticket. */
export function RecoveredCloudDrafts({ backend = cloudRecoveryBackend, enabled = isTauri() }: { backend?: RecoveryBackend; enabled?: boolean }) {
  const [drafts, setDrafts] = useState<RecoveredDraft[]>([]);
  const [next, setNext] = useState<number | null>(null);
  const [listError, setListError] = useState(false), [loading, setLoading] = useState(false);
  const [selected, setSelected] = useState<RecoveredDraft | null>(null);
  const [body, setBody] = useState<RecoveredDraftBody | null>(null), [readError, setReadError] = useState<string | null>(null);
  const current = useRef({ backend, selected, enabled }); current.current = { backend, selected, enabled };
  const mounted = useRef(true), listAction = useRef(0), readAction = useRef(0);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; listAction.current++; readAction.current++; };
  }, []);
  async function load(offset = 0) {
    const action = ++listAction.current;
    setLoading(true); setListError(false);
    try {
      const page = await backend.list(offset);
      if (!mounted.current || action !== listAction.current || current.current.backend !== backend || !current.current.enabled) return;
      setDrafts(prior => offset === 0 ? page.drafts : [...prior, ...page.drafts.filter(draft => !prior.some(entry => entry.draftId === draft.draftId))]);
      setNext(page.nextOffset);
    } catch {
      if (mounted.current && action === listAction.current && current.current.backend === backend) setListError(true);
    } finally { if (mounted.current && action === listAction.current && current.current.backend === backend) setLoading(false); }
  }
  useEffect(() => {
    listAction.current++; readAction.current++;
    setDrafts([]); setSelected(null); setBody(null); setReadError(null); setNext(null); setListError(false);
    if (enabled) void load();
    else setLoading(false);
  }, [backend, enabled]);
  function open(draft: RecoveredDraft) {
    const action = ++readAction.current;
    setSelected(draft); setBody(null); setReadError(null);
    void backend.read(draft).then(result => {
      if (!mounted.current || action !== readAction.current || current.current.backend !== backend || !current.current.enabled) return;
      if (result.draft.draftId !== draft.draftId || result.draft.revision !== draft.revision) throw new Error('The retained draft changed.');
      setBody(result);
    }).catch(() => {
      if (mounted.current && action === readAction.current && current.current.backend === backend)
        setReadError('This retained draft could not be opened. Close this view and reload the draft list.');
    });
  }
  function close() { readAction.current++; setSelected(null); setBody(null); setReadError(null); }
  if (!enabled || !drafts.length && !listError && !loading) return null;
  return <section className="provider-edit-drafts" aria-label="Recover retained cloud drafts">
    <details className="recovered-cloud-drafts">
    <summary>Recover cloud drafts…</summary>
    {drafts.map(draft => <button type="button" className="settings-link-button" key={draft.draftId} onClick={() => open(draft)} title={draft.sourceName}>
      {draft.title} · Save snapshot
    </button>)}
    {loading ? <p role="status">Loading retained drafts…</p> : null}
    {listError ? <><p role="alert">Retained drafts could not be loaded.</p><button type="button" className="settings-link-button" disabled={loading} onClick={() => void load()}>Reload drafts</button></>
      : next !== null ? <button type="button" className="settings-link-button" disabled={loading} onClick={() => void load(next)}>More drafts</button> : null}
    </details>
    {selected ? <RecoveredDraftDialog key={`${selected.draftId}:${selected.revision}`} draft={selected} body={body} error={readError} onClose={close} /> : null}
  </section>;
}
