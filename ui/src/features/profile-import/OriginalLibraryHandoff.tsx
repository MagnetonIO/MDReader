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
