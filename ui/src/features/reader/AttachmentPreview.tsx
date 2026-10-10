import type { AssetPreview } from '../../bindings';
import type { LocalImageBytes } from './contracts';
import { LocalImageProvider, LocalSourceImage, useLocalImages } from './LocalSourceImage';
import { checkedRasterBytes } from './attachment-preview';
import './attachment-preview.css';

async function raster(preview: AssetPreview): Promise<LocalImageBytes> {
  return checkedRasterBytes(preview);
}

/** Text stays escaped React text; no HTML, SVG, embedded file or OS execution. */
export function AttachmentPreview({ preview, onClose }: { preview: AssetPreview; onClose: () => void }) {
  const images = useLocalImages(preview, raster, preview.content.kind === 'raster');
  return <section className="attachment-preview" aria-label="Linked file preview">
    <header className="attachment-preview__bar">
      <button type="button" autoFocus onClick={onClose}>Return to document</button>
      <strong>{preview.path.split('/').at(-1)}</strong>
      <span>Read only</span>
    </header>
    {preview.content.kind === 'text'
      ? <><p>{preview.content.format} · {preview.path}</p>
        {preview.fragment ? <p role="status">Showing the complete linked file. Markdown heading navigation is unavailable in this text preview.</p> : null}
        <pre className="attachment-preview__text" tabIndex={0}>{preview.content.text}</pre></>
      : <LocalImageProvider images={images}><LocalSourceImage src="preview" alt={preview.path.split('/').at(-1)} /></LocalImageProvider>}
  </section>;
}
