import { useLayoutEffect, useRef } from 'react';
import { fileContext } from '@mdreader/ui';
import { CloseIcon } from './icons';
import type { DocRef } from '../stores/types';
import './workspace-link-choices.css';

/** Presentation only: every selection returns the original, exact candidate. */
export function WorkspaceLinkChoices({ candidates, root, onChoose, onDismiss, canRestoreFocus }: {
  candidates: readonly DocRef[];
  root: string | null;
  onChoose(candidate: DocRef): void;
  onDismiss(): void;
  canRestoreFocus(): boolean;
}) {
  const host = useRef<HTMLElement>(null);
  const chosen = useRef(false);
  const restore = useRef(canRestoreFocus);
  restore.current = canRestoreFocus;
  useLayoutEffect(() => {
    const origin = document.activeElement;
    host.current?.querySelector<HTMLButtonElement>('.workspace-link-choices__item')?.focus({ preventScroll: true });
    return () => {
      if (!chosen.current && restore.current() && origin instanceof HTMLElement &&
        origin.isConnected && !origin.closest('[inert], [hidden], [aria-hidden="true"]')) {
        origin.focus({ preventScroll: true });
      }
    };
  }, []);
  return <section ref={host} className="workspace-link-choices" aria-label="Choose linked document"
    onKeyDown={event => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onDismiss(); }
    }}>
    <div className="workspace-link-choices__header">
      <h2>Choose a note</h2>
      <button type="button" className="icon-btn" aria-label="Dismiss linked document choices" onClick={onDismiss}>
        <CloseIcon width={16} height={16} />
      </button>
    </div>
    <ul>
      {candidates.map(candidate => {
        const context = fileContext(candidate.path, root);
        const name = candidate.path.split(/[\\/]/).pop() || candidate.name;
        const parent = context.relativePath ? context.parentLabel : 'Selected source';
        return <li key={candidate.path}>
          <button type="button" className="workspace-link-choices__item" aria-label={`${name} — ${parent}`}
            title={context.relativePath ?? name}
            onClick={() => { chosen.current = true; onChoose(candidate); }}>
            <strong>{name}</strong><span>{parent}</span>
          </button>
        </li>;
      })}
    </ul>
  </section>;
}
