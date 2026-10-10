import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { useRef } from 'react';
import { MarkdownReader } from '@mdreader/ui';
import type { MarkdownReaderHandle } from '@mdreader/ui';
import { parseMarkdown } from '@mdreader/markdown';
import { useAppearanceChange } from './useAppearanceChange';
import { readingAppearance } from './appearance';
import { useSettingsStore, textScaleFactor } from '../../stores/settings';
const doc = parseMarkdown('# Public title\n\nFirst paragraph.\n\nSecond paragraph.');
afterEach(() => { cleanup(); vi.restoreAllMocks(); useSettingsStore.getState().reset(); });

test('font/size reflow restores the same visible block without remounting or rereading Markdown', () => {
  function Example({ identity }: { identity: string }) {
    const reader = useRef<MarkdownReaderHandle>(null);
    const font = useSettingsStore(state => state.readingFont);
    const size = useSettingsStore(state => state.textScaleIndex);
    const change = useAppearanceChange(reader, identity);
    return <><button onClick={() => change(() => useSettingsStore.getState().increaseTextSize())}>Resize</button>
      <div style={readingAppearance(font, textScaleFactor({ textScaleIndex: size }))}><MarkdownReader doc={doc} ref={reader} /></div></>;
  }
  const view = render(<Example identity="public-a" />);
  const scroll = view.container.querySelector<HTMLElement>('.mdreader')!;
  const blocks = Array.from(scroll.querySelectorAll<HTMLElement>('[data-block-index]'));
  vi.spyOn(scroll, 'getBoundingClientRect').mockReturnValue({ top: 0 } as DOMRect);
  for (const [index, block] of blocks.entries()) vi.spyOn(block, 'getBoundingClientRect').mockImplementation(() => ({
    top: 32 + index * (useSettingsStore.getState().textScaleIndex === 3 ? 100 : 150) - scroll.scrollTop,
  } as DOMRect));
  scroll.scrollTop = 132;
  fireEvent.click(screen.getByRole('button', { name: 'Resize' }));
  expect(view.container.querySelector('.mdreader')).toBe(scroll);
  expect(scroll.scrollTop).toBe(182);
  expect(blocks[1]!.getBoundingClientRect().top).toBe(0);
});
