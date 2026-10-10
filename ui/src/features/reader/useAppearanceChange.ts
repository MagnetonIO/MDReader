import { useLayoutEffect, useRef } from 'react';
import type { RefObject } from 'react';
import type { MarkdownReaderHandle, ScrollAnchor } from '@mdreader/ui';
import { useSettingsStore } from '../../stores/settings';

/** Capture the visible block before reflow, restore after the appearance commit.
 * Selection changes retire the request; no read, parse or source mutation. */
export function useAppearanceChange(reader: RefObject<MarkdownReaderHandle | null>, selection: unknown, onIntent?: () => void) {
  const font = useSettingsStore(state => state.readingFont);
  const size = useSettingsStore(state => state.textScaleIndex);
  const pending = useRef<{ selection: unknown; anchor: ScrollAnchor } | null>(null);
  const selected = useRef(selection); selected.current = selection;
  useLayoutEffect(() => {
    const restore = pending.current; pending.current = null;
    if (restore && restore.selection === selection) reader.current?.scrollToBlock(restore.anchor.block, restore.anchor.offset);
  }, [font, size, selection, reader]);
  return (change: () => void) => {
    onIntent?.();
    const anchor = reader.current?.getScrollAnchor();
    pending.current = anchor ? { selection: selected.current, anchor } : null;
    change();
  };
}
