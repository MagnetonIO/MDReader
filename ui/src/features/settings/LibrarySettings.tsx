import { FolderIcon } from '../../shell/icons.js';
import { activityLabel, sourceState } from './presentation.js';
import type { SettingsController } from './useSettingsController.js';

export function LibrarySettings({ controller }: { controller: SettingsController }) {
  const { library, failure, busy, addSource, watcher, importState, activate, forget } = controller;
  return (<section className="settings-section" aria-labelledby="settings-library">
    <div className="settings-section__intro">
      <div>
        <h2 id="settings-library">Library &amp; sources</h2>
      </div>
      <p>
        Add the Markdown sources you want to read and search as one Library.
      </p>
    </div>

    <div className="settings-callout">
      <FolderIcon />
      <div>
        <strong>Local Markdown folders</strong>
        <span>Membership is remembered; your files stay where you put them.</span>
      </div>
      <button type="button" disabled={busy !== null} onClick={addSource}>
        {busy === 'add' ? 'Choosing…' : 'Add folder'}
      </button>
    </div>

    <div className="library-list" aria-live="polite">
      {!library && !failure ? <div className="settings-empty">Reading local library…</div> : null}
      {library?.sources.length === 0 ? (
        <div className="settings-empty">No folders yet. Add one to begin.</div>
      ) : null}
      {library?.sources.map((source) => {
        const state = sourceState(source);
        return (
          <article
            key={source.id}
            className={`library-source${source.active ? ' library-source--active' : ''}`}
          >
            <div className="library-source__rail" aria-hidden="true" />
            <div className="library-source__body">
              <div className="library-source__heading">
                <div>
                  <h3>{source.name}</h3>
                  <div className="library-source__path" title={source.path}>
                    {source.path}
                  </div>
                </div>
                {source.active ? <span className="settings-badge">Active</span> : null}
              </div>
              <div className="library-source__meta">
                <span className={state.unavailable ? 'settings-state--warning' : ''}>
                  {state.label}
                </span>
                <span>
                  {source.index_state === 'present'
                    ? 'Index on disk'
                    : source.index_state === 'not-created'
                      ? 'Index not created'
                      : 'Index status unavailable'}
                </span>
                {source.active ? <span>{activityLabel(watcher, importState)}</span> : null}
              </div>
              <div className="library-source__actions">
                {!source.active ? (
                  <button
                    type="button"
                    aria-label={`Activate ${source.name}`}
                    disabled={state.unavailable || busy !== null}
                    onClick={() => activate(source)}
                  >
                    Activate
                  </button>
                ) : null}
                <button
                  type="button"
                  className="settings-button--quiet"
                  aria-label={`Forget ${source.name}`}
                  disabled={busy !== null}
                  title={
                    source.active
                      ? 'Closes this source, removes its private MDReader index, and keeps every file'
                      : 'Removes this entry and its local MDReader index; your files stay'
                  }
                  onClick={(event) => forget(source, event.currentTarget)}
                >
                  {busy === `forget:${source.id}` ? 'Forgetting…' : 'Forget'}
                </button>
              </div>
            </div>
          </article>
        );
      })}
    </div>
    {library?.cleanup.kind === 'pending' ? (
      <p className="settings-footnote settings-state--warning" role="status">
        Finishing local cleanup for {library.cleanup.source_count}{' '}
        {library.cleanup.source_count === 1 ? 'source' : 'sources'}. Your folders are not
        affected.
      </p>
    ) : null}
    <p className="settings-footnote">
      Forget removes the library entry and MDReader’s private index. It never deletes the
      folder or its files.
    </p>

  </section>
  );
}
