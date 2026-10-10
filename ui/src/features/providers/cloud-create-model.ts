import type { ProviderSource } from '../../platform/providers';

export function canCreateCloudDocument(source: ProviderSource): boolean {
  return source.enabled && source.health === 'ready' && source.capabilities.guardedWrite &&
    (source.selectedScope.kind === 'drive_folder' || source.selectedScope.kind === 'scoped_root');
}
export function cloudDestination(source: ProviderSource): string {
  const scope = source.selectedScope;
  if (scope.kind === 'drive_folder') return scope.folderId === 'root' ? 'My Drive' : 'Selected Drive folder';
  if (scope.kind === 'scoped_root') return scope.path ?? 'Selected Dropbox folder';
  return 'Selected connection';
}
export function cloudMarkdownFilename(input: string): string | null {
  const name = input.trim();
  if (!name || /[/\\\x00-\x1f\x7f-\x9f]/.test(name)) return null;
  const filename = /\.(md|markdown)$/i.test(name) ? name : `${name}.md`;
  return new TextEncoder().encode(filename).length <= 240 && !/^\.(md|markdown)$/i.test(filename) ? filename : null;
}
