import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { MobileReaderFrame, type MobileReaderFrameProps } from './MobileReaderFrame';
import { useSettingsStore } from '../stores/settings';

afterEach(() => { cleanup(); useSettingsStore.getState().reset(); });
const props: MobileReaderFrameProps = {
  identity: 'public:revision-a', title: 'A long public Markdown title at phone width.md', onClose: vi.fn(),
  actions: <button type="button">Pin</button>, details: <p>Exact public revision</p>,
  outline: [{ slug: 'section', text: 'Section', depth: 2, lineStart: 21 }], onOutline: vi.fn(),
  onAppearanceChange: change => change(), children: <div className="public-reader" tabIndex={0}>Public body</div>,
};

test('compact header leaves actions in a bounded modal while preserving the reader node and place', () => {
  const view = render(<MobileReaderFrame {...props} />);
  const reader = view.container.querySelector<HTMLElement>('.public-reader')!;
  reader.scrollTop = 97;
  const trigger = screen.getByRole('button', { name: 'Reader options' });
  expect(screen.getAllByRole('button').map(button => button.textContent)).toEqual(['Back', '•••']);
  expect(screen.queryByRole('button', { name: 'Pin' })).toBeNull();
  fireEvent.click(trigger);
  expect(trigger.getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByRole('dialog', { name: 'Reader options' })).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Close reader options' }));
  expect(reader.closest('[inert]')).toBeTruthy();
  expect(reader.closest('[hidden]')).toBeNull();
  fireEvent.keyDown(document.activeElement!, { key: 'Escape' });
  expect(trigger.getAttribute('aria-expanded')).toBe('false');
  expect(document.activeElement).toBe(trigger);
  expect(view.container.querySelector('.public-reader')).toBe(reader);
  expect(reader.scrollTop).toBe(97);
  expect(reader.closest('[inert]')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Back' }));
  expect(props.onClose).toHaveBeenCalledOnce();
});

test('tabs support arrow navigation, focus trapping and intentional outline navigation', () => {
  const onOutline = vi.fn();
  render(<MobileReaderFrame {...props} onOutline={onOutline} />);
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  const actions = screen.getByRole('tab', { name: 'Actions' }); actions.focus();
  fireEvent.keyDown(actions, { key: 'ArrowRight' });
  const outline = screen.getByRole('tab', { name: 'Outline' });
  expect(document.activeElement).toBe(outline);
  expect(outline.getAttribute('aria-selected')).toBe('true');
  const heading = screen.getByRole('button', { name: 'Section' }); heading.focus();
  fireEvent.keyDown(heading, { key: 'Tab' });
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Close reader options' }));
  fireEvent.click(heading);
  expect(onOutline).toHaveBeenCalledExactlyOnceWith(21);
  expect(screen.queryByRole('dialog')).toBeNull();
});

test('selection changes close old options without returning focus to another document', () => {
  const view = render(<MobileReaderFrame {...props} />);
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  const trigger = screen.getByRole('button', { name: 'Reader options', hidden: true });
  const focus = vi.spyOn(trigger, 'focus');
  view.rerender(<MobileReaderFrame {...props} identity="public:revision-b" title="Next public copy" />);
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(focus).not.toHaveBeenCalled();
});

test('appearance uses the existing store and size bounds without enabling source actions', () => {
  render(<MobileReaderFrame {...props} />);
  fireEvent.click(screen.getByRole('button', { name: 'Reader options' }));
  fireEvent.click(screen.getByRole('tab', { name: 'Appearance' }));
  fireEvent.click(screen.getByRole('radio', { name: 'Sans serif' }));
  expect(useSettingsStore.getState().readingFont).toBe('sans');
  fireEvent.click(screen.getByRole('button', { name: 'Larger text' }));
  expect(screen.getByLabelText('Text size').textContent).toBe('108%');
  for (let n = 0; n < 8; n++) fireEvent.click(screen.getByRole('button', { name: 'Larger text' }));
  expect((screen.getByRole('button', { name: 'Larger text' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.queryByRole('button', { name: 'Pin' })).toBeNull();
});
