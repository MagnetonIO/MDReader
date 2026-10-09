export type OriginalLibraryWait = 'original_open' | 'background_unsupported' | 'handoff_unconfirmed';

/** These states are native observations, never guesses from window visibility. */
export function OriginalLibraryHandoff({ reason }: { reason: OriginalLibraryWait }) {
  const copy = reason === 'background_unsupported' ? {
    title: 'The original background service cannot finish the handoff',
    message: 'The original window may already be closed. Its background service cannot safely finish this import with this version.',
    action: 'Keep your originals. Leave this import pending until the original service can be shut down normally or updated, then choose Retry import here.',
  } : reason === 'handoff_unconfirmed' ? {
    title: 'The original library handoff is not confirmed',
    message: 'The original window may already be closed, but the background service has not confirmed that the library is ready to copy.',
    action: 'Keep your originals and any drafts. Retry import only checks the same pending handoff; it may remain blocked until the original service can safely resume or shut down.',
  } : {
    title: 'Close the original MDReader app',
    message: 'Return to the original app and quit it normally. Save or keep any drafts when it asks. Then come back here and choose Retry import.',
    action: 'This window stays open. Your selected original library is remembered; you do not need to choose it again.',
  };
  return <section className="profile-import__close-original" aria-label={copy.title}>
    <h4>{copy.title}</h4>
    <p>{copy.message}</p>
    <p>{copy.action}</p>
  </section>;
}

/** A rollback handoff keeps its phase even when the original reply is lost. */
export function ReturnLibraryHandoff() {
  return <section className="profile-import__close-original" aria-label="Return to original library pending">
    <h4>The return to your original library is pending</h4>
    <p>The background service has not confirmed the handoff. Your original library and separate copy are kept; this is not a completed return.</p>
    <p>Retry return to original library checks the same operation. It may remain blocked until the service can safely finish the handoff.</p>
  </section>;
}
