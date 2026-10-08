import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SettingsSheet } from '../../shell/SettingsSheet.js';
import type { SettingsBackend } from '../../platform/settings.js';
import { visibleSettingsControls } from './focus.js';

vi.mock('../../shell/IndexingPanel.js', () => ({ IndexingPanel: () => <p>Indexing controls</p> }));
vi.mock('../../shell/AgentCapturePanel.js', () => ({ AgentCapturePanel: () => <p>Capture controls</p> }));
vi.mock('../../shell/NativeProvidersPanel.js', () => ({ NativeProvidersPanel: () => <button>Manage selected provider</button> }));
afterEach(cleanup);

function mount() {
  const backend = {
    listSources: vi.fn(async () => ({ sources: [], cleanup: { kind: 'complete' } })),
    diagnosticsInfo: vi.fn(async () => ({ level: 'standard', max_file_bytes: 1024, max_retained_files: 3, max_export_bytes: 4096 })),
  } as unknown as SettingsBackend;
  render(<SettingsSheet backend={backend} watcher="idle" importState={null}
    onActivateSource={async () => true} onForgetActiveSource={async () => {}}
    onOpenProviderDocument={async () => {}} onClose={vi.fn()} />);
  return backend;
}

describe('Settings category lifecycle', () => {
  it('shows one category while mounted owners and backend state survive section changes', async () => {
    const backend = mount();
    await screen.findByText('Stored locally on this device');
    const dialog = screen.getByRole('dialog', { name: 'Settings' });
    expect(screen.getByRole('heading', { name: 'Appearance' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Manage selected provider' })).toBeNull();
    const nav = within(screen.getByRole('navigation', { name: 'Settings sections' }));
    fireEvent.click(nav.getByRole('button', { name: 'Account & sync' }));
    expect(screen.getByRole('button', { name: 'Manage selected provider' })).toBeTruthy();
    expect(screen.queryByRole('heading', { name: 'Appearance' })).toBeNull();
    expect(nav.getByRole('button', { name: 'Account & sync' }).getAttribute('aria-current')).toBe('page');
    fireEvent.click(nav.getByRole('button', { name: 'Advanced' }));
    expect(screen.getByText('Capture controls')).toBeTruthy();
    expect(screen.getByText('Indexing controls')).toBeTruthy();
    expect(visibleSettingsControls(dialog).some((node) => node.textContent === 'Manage selected provider')).toBe(false);
    expect(backend.listSources).toHaveBeenCalledOnce();
    expect(backend.diagnosticsInfo).toHaveBeenCalledOnce();
  });

  it('excludes hidden sections and closed migration controls from both ends of the focus loop', async () => {
    mount();
    await screen.findByText('Stored locally on this device');
    const dialog = screen.getByRole('dialog', { name: 'Settings' });
    fireEvent.click(screen.getByRole('button', { name: 'Advanced' }));
    const hiddenControl = document.createElement('button');
    hiddenControl.textContent = 'Responsive hidden control';
    dialog.append(hiddenControl);
    hiddenControl.style.display = 'none';
    expect(visibleSettingsControls(dialog)).not.toContain(hiddenControl);
    hiddenControl.style.display = '';
    hiddenControl.style.visibility = 'hidden';
    expect(visibleSettingsControls(dialog)).not.toContain(hiddenControl);
    const controls = visibleSettingsControls(dialog);
    const last = controls.at(-1)!;
    expect(last.tagName).toBe('SUMMARY');
    last.focus();
    fireEvent.keyDown(last, { key: 'Tab' });
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Close Settings' }));
    fireEvent.keyDown(document.activeElement!, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(last);
  });
});
