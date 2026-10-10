import { useId, useLayoutEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { CloseIcon } from '../shell/icons';
import { MobileReadingAppearance } from './MobileReadingAppearance';
import { useReaderSheetIsolation } from './useReaderSheetIsolation';
import './mobile-reader-frame.css';

type Panel = 'Actions' | 'Outline' | 'Appearance' | 'Details';
const PANELS: readonly Panel[] = ['Actions', 'Outline', 'Appearance', 'Details'];
export interface MobileReaderFrameProps {
  identity: string;
  title: string;
  closeLabel?: string;
  onClose(): void;
  actions: ReactNode;
  details: ReactNode;
  outline: readonly { slug: string; text: string; depth: number; lineStart: number }[];
  onOutline(line: number): void;
  onAppearanceChange(change: () => void): void;
  children: ReactNode;
}

/** Phone navigation stays compact. Secondary reading controls occupy a bounded
 * modal sheet; the inert document keeps its layout and virtualizer mounted. */
export function MobileReaderFrame({ identity, title, closeLabel = 'Back', onClose, actions, details, outline, onOutline, onAppearanceChange, children }: MobileReaderFrameProps) {
  const id = useId();
  const [selection, setSelection] = useState<{ identity: string; panel: Panel } | null>(null);
  const open = selection?.identity === identity;
  const panel = open ? selection.panel : 'Actions';
  const trigger = useRef<HTMLButtonElement>(null);
  const frame = useRef<HTMLDivElement>(null);
  useReaderSheetIsolation(frame, open);
  const dialog = useRef<HTMLElement>(null);
  const previous = useRef<{ identity: string; open: boolean }>({ identity, open: false });
  useLayoutEffect(() => {
    if (open && !previous.current.open) dialog.current?.querySelector<HTMLElement>('button')?.focus({ preventScroll: true });
    else if (!open && previous.current.open && previous.current.identity === identity && trigger.current?.isConnected)
      trigger.current.focus({ preventScroll: true });
    previous.current = { identity, open };
  }, [open, identity]);
  const dismiss = () => setSelection(null);
  return <div ref={frame} className="mobile-reader-frame">
    <div className="mobile-reader-frame__origin" inert={open} aria-hidden={open ? true : undefined}>
      <header className="mobile-reader__bar">
        <button type="button" className="mobile-touch" aria-label={closeLabel} onClick={onClose}>Back</button>
        <strong title={title}>{title}</strong>
        <button ref={trigger} type="button" className="mobile-touch mobile-reader-frame__overflow"
          aria-label="Reader options" aria-haspopup="dialog" aria-expanded={open} aria-controls={`${id}-sheet`}
          onClick={() => setSelection({ identity, panel: 'Actions' })}>
          <span aria-hidden="true">•••</span>
        </button>
      </header>
      <div className="mobile-reader-frame__document">{children}</div>
    </div>
    <div className="mobile-reader-frame__backdrop" hidden={!open} onClick={event => { if (event.target === event.currentTarget) dismiss(); }}>
      <section ref={dialog} id={`${id}-sheet`} className="mobile-reader-frame__sheet" role="dialog" aria-modal="true"
        aria-labelledby={`${id}-title`} onKeyDown={event => {
          if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); dismiss(); }
          if (event.key !== 'Tab') return;
          const items = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), summary, a[href], [tabindex="0"]') ?? [])
            .filter(element => !element.closest('[hidden]') && element.tabIndex !== -1);
          const first = items[0], last = items.at(-1);
          if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
          else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
        }}>
        <header><h2 id={`${id}-title`}>Reader options</h2><button type="button" aria-label="Close reader options" onClick={dismiss}><CloseIcon /></button></header>
        <div className="mobile-reader-frame__tabs" role="tablist" aria-label="Reader controls">
          {PANELS.map(value => <button type="button" role="tab" key={value}
            id={`${id}-${value}`} aria-controls={`${id}-${value}-panel`} aria-selected={panel === value}
            tabIndex={panel === value ? 0 : -1}
            onKeyDown={event => {
              const index = PANELS.indexOf(value);
              const next = event.key === 'ArrowRight' ? PANELS[(index + 1) % PANELS.length]
                : event.key === 'ArrowLeft' ? PANELS[(index + PANELS.length - 1) % PANELS.length]
                  : event.key === 'Home' ? PANELS[0] : event.key === 'End' ? PANELS.at(-1) : undefined;
              if (!next) return;
              event.preventDefault(); setSelection({ identity, panel: next });
              dialog.current?.querySelector<HTMLElement>(`[id="${id}-${next}"]`)?.focus();
            }}
            onClick={() => setSelection({ identity, panel: value })}>{value}</button>)}
        </div>
        <div className="mobile-reader-frame__panels">
          <section role="tabpanel" id={`${id}-Actions-panel`} aria-labelledby={`${id}-Actions`} hidden={panel !== 'Actions'}>{actions}</section>
          <section role="tabpanel" id={`${id}-Outline-panel`} aria-labelledby={`${id}-Outline`} hidden={panel !== 'Outline'}>
            {outline.length ? <nav aria-label="Document outline">{outline.map(item => <button type="button" key={item.slug}
              style={{ paddingInlineStart: `${Math.min(item.depth, 6) * 0.6}rem` }} onClick={() => { dismiss(); onOutline(item.lineStart); }}>{item.text}</button>)}</nav>
              : <p>No outline available.</p>}
          </section>
          <section role="tabpanel" id={`${id}-Appearance-panel`} aria-labelledby={`${id}-Appearance`} hidden={panel !== 'Appearance'}>
            <MobileReadingAppearance onChange={onAppearanceChange} />
          </section>
          <section role="tabpanel" id={`${id}-Details-panel`} aria-labelledby={`${id}-Details`} hidden={panel !== 'Details'}>{details}</section>
        </div>
      </section>
    </div>
  </div>;
}
