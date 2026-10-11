import { useMemo, useRef, useState, type ComponentType } from 'react';
import { MarkdownReader, PlatformProvider, type MarkdownReaderHandle } from '@mdreader/ui';
import { useSettingsStore, textScaleFactor } from '../../stores/settings';
import { readingAppearance } from '../reader/appearance';
import { createReaderPlatform } from '../../platform/reader-platform';
import { LocalSourceImage } from '../reader/LocalSourceImage';
import { useDraftPreview } from './useDraftPreview';

/** Uses the same sanitized parser/reader; asset access is supplied by the source owner. */
export function DraftPreview({ identity, markdown, Image = LocalSourceImage }: {
  identity: unknown; markdown: string; Image?: ComponentType<{ src?: string; alt?: string }>;
}) {
  const { doc, error } = useDraftPreview(identity, markdown, true);
  const reader = useRef<MarkdownReaderHandle | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const font = useSettingsStore(state => state.readingFont);
  const scale = useSettingsStore(state => state.textScaleIndex);
  const platform = useMemo(() => createReaderPlatform(null, setNotice), []);
  return <div className="editor-views__preview" style={readingAppearance(font, textScaleFactor(scale))} aria-label="Draft preview" aria-busy={!doc && !error}>
    {error ? <p role="alert">{error}</p> : !doc ? <p role="status">Updating preview…</p> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {doc ? <PlatformProvider platform={platform}><MarkdownReader ref={reader} doc={doc} components={{ img: Image }} onNavigate={event => {
      if (event.kind === 'anchor') {
        if (!reader.current?.scrollToFragment(event.slug)) setNotice('The requested location is unavailable in this draft.');
      } else setNotice('View the saved version to open a linked document. Your draft is kept.');
    }} /></PlatformProvider> : null}
  </div>;
}
