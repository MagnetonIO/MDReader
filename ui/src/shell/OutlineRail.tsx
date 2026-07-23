/** The right-hand outline rail. Wraps the shared @mdreader/ui Outline in a quiet
 * rail with a heading; selecting a heading scrolls the reader. */

import { Outline } from '@mdreader/ui';
import type { OutlineItem } from '@mdreader/markdown';

export interface OutlineRailProps {
  items: OutlineItem[];
  activeSlug: string | undefined;
  onSelect: (item: OutlineItem) => void;
}

export function OutlineRail({ items, activeSlug, onSelect }: OutlineRailProps) {
  if (items.length === 0) return null;
  return (
    <aside className="outline-rail" aria-label="Outline">
      <div className="outline-rail__heading">On this page</div>
      <Outline items={items} activeSlug={activeSlug} onSelect={onSelect} />
    </aside>
  );
}
