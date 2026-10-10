import type { AssetPreview } from '../../bindings';
import type { LocalImageBytes } from './contracts';

/** Route explicit non-Markdown filenames to the native linked-file reader.
 * This selects presentation only; native AST and retained handles own admission. */
export function isAttachmentLink(target: string): boolean {
  let path: string;
  try { path = decodeURIComponent(target.split(/[?#]/, 1)[0]!); }
  catch { return false; }
  if (!path || /[:\\\u0000-\u001f\u007f]/.test(path) || path.startsWith('/')) return false;
  const name = path.split('/').at(-1) ?? '';
  return name.includes('.') && !/\.(?:md|markdown)$/i.test(name);
}

/** Reject unexpected responses before presenting any native bytes. */
export function checkedAssetPreview(value: AssetPreview): AssetPreview {
  if (!value || !value.path || value.path.length > 4096 || value.path.startsWith('/')
    || /[:\\\u0000-\u001f\u007f]/.test(value.path)
    || value.path.split('/').some((part) => !part || part === '.' || part === '..')
    || !/^[a-f0-9]{64}$/.test(value.revision)
    || value.fragment !== null && (typeof value.fragment !== 'string' || value.fragment.length > 4096 || /[\u0000-\u001f\u007f]/.test(value.fragment))) {
    throw new Error('The linked file response could not be verified.');
  }
  const content = value.content;
  if (content?.kind === 'text' && content.mime === 'text/plain'
    && typeof content.format === 'string' && content.format.length <= 64
    && typeof content.text === 'string' && new TextEncoder().encode(content.text).byteLength <= 1024 * 1024) return value;
  if (content?.kind === 'raster' && (content.mime === 'image/png' || content.mime === 'image/jpeg')
    && typeof content.base64 === 'string' && content.base64.length > 0 && content.base64.length <= 5_592_408
    && Number.isInteger(content.width) && Number.isInteger(content.height)
    && content.width > 0 && content.height > 0 && content.width <= 4096 && content.height <= 4096
    && content.width * content.height <= 4_000_000) return value;
  throw new Error('This linked file is outside the supported preview limits.');
}

/** Generated MIME strings become image capabilities only after literal checks. */
export function checkedRasterBytes(preview: AssetPreview): LocalImageBytes {
  checkedAssetPreview(preview);
  const content = preview.content;
  if (content.kind !== 'raster' || (content.mime !== 'image/png' && content.mime !== 'image/jpeg')) {
    throw new Error('This file is not a supported image.');
  }
  return { mime: content.mime, base64: content.base64, width: content.width, height: content.height };
}

export function assetErrorMessage(kind: string): string {
  switch (kind) {
    case 'stale': return 'The document or folder changed. Reload the document before opening its linked file.';
    case 'unauthorized': return 'This linked file is outside the retained folder or is not referenced by the saved document. Save new links before opening them.';
    case 'too_large': return 'This linked file is too large to preview.';
    case 'unsupported': return 'This file type cannot be previewed here. Text files and PNG or JPEG images are supported.';
    case 'busy': return 'A linked file is still loading. Try again in a moment.';
    case 'invalid': return 'This linked file address is invalid.';
    default: return 'The linked file could not be found or read in this folder.';
  }
}
