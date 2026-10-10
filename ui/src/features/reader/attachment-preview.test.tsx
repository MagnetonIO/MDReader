import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { AttachmentPreview } from './AttachmentPreview';
import { AttachmentLayer } from './AttachmentLayer';
import { checkedAssetPreview, isAttachmentLink } from './attachment-preview';
import type { AssetPreview } from '../../bindings';

afterEach(() => cleanup());
const text: AssetPreview = { path: 'assets/public.drawio', revision: 'a'.repeat(64), fragment: null,
  content: { kind: 'text', mime: 'text/plain', format: 'xml', text: '<script>window.secret = 1</script><mxfile />' } };

test('linked XML and HTML-like bytes are escaped inert text and Return preserves the caller', () => {
  const close = vi.fn();
  const view = render(<AttachmentPreview preview={checkedAssetPreview(text)} onClose={close} />);
  expect(view.container.querySelector('pre')!.textContent).toBe(text.content.kind === 'text' ? text.content.text : '');
  expect(view.container.querySelector('script, iframe, object, embed, mxfile')).toBeNull();
  expect(screen.getByText('Read only')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Return to document' }));
  expect(close).toHaveBeenCalledOnce();
});

test('only explicit non-Markdown filenames select attachment presentation; URI and Markdown routes stay separate', () => {
  for (const target of ['../assets/a%20b.sql', 'public.drawio', 'config.yaml#L2', 'diagram.png']) expect(isAttachmentLink(target)).toBe(true);
  for (const target of ['Next%20note.md#repeated-1', 'README.MARKDOWN', 'Shared title', 'https://example.com/a.sql', '/a.sql', '%00a.sql', '%xx.sql']) expect(isAttachmentLink(target)).toBe(false);
});

test('unexpected preview paths, executable content kinds and oversized bytes fail closed', () => {
  expect(() => checkedAssetPreview({ ...text, path: '../outside.sql' })).toThrow();
  expect(() => checkedAssetPreview({ ...text, content: { kind: 'html', mime: 'text/html', text: '<b>unsafe</b>' } } as unknown as AssetPreview)).toThrow();
  expect(() => checkedAssetPreview({ ...text, content: { kind: 'text', mime: 'text/plain', format: 'sql', text: 'x'.repeat(1024 * 1024 + 1) } })).toThrow();
});

test('a changed document origin cannot restore the old link focus after removing its preview', () => {
  const first = {};
  const view = render(<AttachmentLayer origin={first} preview={null} onClose={() => {}}><button>Original link</button></AttachmentLayer>);
  const link = screen.getByRole('button', { name: 'Original link' });
  link.focus();
  const focus = vi.spyOn(link, 'focus');
  view.rerender(<AttachmentLayer origin={first} preview={text} onClose={() => {}}><button>Original link</button></AttachmentLayer>);
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Return to document' }));
  view.rerender(<AttachmentLayer origin={{}} preview={null} onClose={() => {}}><button>New document link</button></AttachmentLayer>);
  expect(focus).not.toHaveBeenCalled();
  expect(document.activeElement).not.toBe(link);
});

test('Return falls back to the retained reader without scrolling when the original link disappeared', () => {
  const origin = {};
  const view = render(<AttachmentLayer origin={origin} preview={null} onClose={() => {}}><button>Original link</button></AttachmentLayer>);
  const link = screen.getByRole('button', { name: 'Original link' });
  link.focus();
  const oldFocus = vi.spyOn(link, 'focus');
  view.rerender(<AttachmentLayer origin={origin} preview={text} onClose={() => {}}><button>Original link</button></AttachmentLayer>);
  const retained = view.container.querySelector<HTMLElement>('.attachment-layer__origin')!;
  const fallback = vi.spyOn(retained, 'focus');
  view.rerender(<AttachmentLayer origin={origin} preview={null} onClose={() => {}}><p>Reader content remains</p></AttachmentLayer>);
  expect(oldFocus).not.toHaveBeenCalled();
  expect(fallback).toHaveBeenCalledWith({ preventScroll: true });
  expect(document.activeElement).toBe(retained);
});
