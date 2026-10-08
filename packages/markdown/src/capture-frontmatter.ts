import type { Root, Yaml } from 'mdast';

/** Capture v1 puts its identifying comment before YAML. CommonMark otherwise
 * interprets that YAML as a setext heading. Recognize only this exact envelope;
 * this is presentation, not verification of the supplied capture provenance.
 * Keep the existing AST positions so reading evidence still names raw lines.
 */
export function recognizeCaptureFrontmatter(tree: Root, source: string): void {
  const match = /^<!-- mdreader-capture:v1 -->\r?\n---\r?\n(mdreader_capture: ([^\r\n]+))\r?\n---\r?\n(?:\r?\n|$)/.exec(source);
  if (!match) return;
  try {
    const metadata: unknown = JSON.parse(match[2]!);
    if (!metadata || typeof metadata !== 'object'
      || !('schema_version' in metadata) || metadata.schema_version !== 1
      || !('capture_is_not_approval' in metadata) || metadata.capture_is_not_approval !== true
      || !('occurrences' in metadata) || !Array.isArray(metadata.occurrences) || !metadata.occurrences.length) return;
  } catch { return; }
  const [marker, delimiter, heading] = tree.children;
  if (marker?.type !== 'html' || marker.value.trim() !== '<!-- mdreader-capture:v1 -->'
    || delimiter?.type !== 'thematicBreak' || delimiter.position?.start.line !== 2
    || heading?.type !== 'heading' || heading.depth !== 2
    || heading.position?.start.line !== 3 || heading.position.end.line !== 4) return;
  const yaml: Yaml = {
    type: 'yaml', value: match[1]!,
    position: { start: delimiter.position.start, end: heading.position.end },
  };
  tree.children.splice(0, 3, yaml);
}
