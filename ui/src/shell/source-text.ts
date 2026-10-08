/** CodeMirror stores logical lines. Keep raw owner buffers unchanged until a
 * user edit, then serialize with the document's existing newline convention. */
import { Text } from '@codemirror/state';

export function sourceLineSeparator(value: string): string {
  return /\r\n|\r|\n/.exec(value)?.[0] ?? '\n';
}
export function sourceText(value: string): Text {
  // This splits separators only. In particular, a leading BOM stays content.
  return Text.of(value.split(/\r\n|\r|\n/));
}
