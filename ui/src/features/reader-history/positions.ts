import type { ParsedDoc } from '@mdreader/markdown';
import type { ScrollAnchor } from '@mdreader/ui';
import type { ReaderPosition } from './model';
import { parsePosition } from './serialization';
function blocks(doc: ParsedDoc) { return doc.hast.children.filter((node) => node.type === 'element'); }
export function savedAnchor(doc: ParsedDoc, revision: string, anchor: ScrollAnchor): ReaderPosition | null {
  const block = blocks(doc)[anchor.block];
  const line = Number(block?.properties.dataLineStart ?? 0);
  return parsePosition({ ...anchor, line, revision }, revision);
}
export function restoreAnchor(doc: ParsedDoc, revision: string, lineEnd: number, position: ReaderPosition | null) {
  if (!position) return { anchor: null, line: null, notice: null };
  if (!parsePosition(position, revision) || position.line > lineEnd) {
    return { anchor: null, line: null, notice: 'The saved position is unavailable for this exact revision. Showing the beginning.' };
  }
  const block = blocks(doc)[position.block];
  if (Number(block?.properties.dataLineStart ?? 0) !== position.line) {
    return { anchor: null, line: position.line, notice: 'The reading layout changed. Restored the saved source line in this exact revision.' };
  }
  return { anchor: { block: position.block, offset: position.offset }, line: position.line, notice: null };
}
