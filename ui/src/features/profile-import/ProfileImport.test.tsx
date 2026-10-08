import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { BackupSettings } from '../settings/BackupSettings';
import type { ImportReply, ImportRequest } from './backend';

const native = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: native }));
const operationId = 'd15ce0ef-141f-4a0a-ae17-fa51ab44895c';
const documents = [
  { documentId: '6e2685df-1377-4c28-a421-7da0f5bf7fb7', name: 'First original.md', revision: 'a'.repeat(64), bytes: 159, pinned: true, line: 7 },
  { documentId: 'e76e9a41-bdc0-481b-943e-1ac5c6ad6329', name: 'Second original.md', revision: 'b'.repeat(64), bytes: 300, pinned: false, line: 1 },
];
const reply = (state: ImportReply['state'], preview = false): ImportReply => ({ state, operationId, documents: preview ? documents : [], imported: preview ? 0 : 1 });
const button = (name: string) => screen.getByRole('button', { name }) as HTMLButtonElement;
function boundary(handler: (request: ImportRequest) => ImportReply | Promise<ImportReply>) {
  native.mockImplementation(async (command: string, args: { request: ImportRequest }) => {
    expect(command).toBe('native_profile_migration');
    return handler(args.request);
  });
}
beforeEach(() => {
  native.mockReset();
  Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true });
});
afterEach(async () => {
  cleanup();
  await Promise.resolve();
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
});

test('preview is user initiated and only explicitly selected exact documents are prepared', async () => {
  boundary((request) => {
    if (request.operation === 'preview') return reply('preview', true);
    if (request.operation === 'prepare') return reply('prepared');
    if (request.operation === 'close') return reply('closed');
    throw new Error('Unexpected native action');
  });
  render(<BackupSettings />);
  expect(native).not.toHaveBeenCalled();
  expect(screen.queryByRole('group', { name: 'Documents to import' })).toBeNull();
  fireEvent.click(button('Import reader data…'));
  const first = await screen.findByRole('checkbox', { name: /First original/ }) as HTMLInputElement;
  expect(first.checked).toBe(false);
  expect((screen.getByRole('checkbox', { name: /Second original/ }) as HTMLInputElement).checked).toBe(false);
  expect(button('Create separate library…').disabled).toBe(true);
  expect(document.activeElement).toBe(screen.getByRole('heading', { name: 'Import reader data' }));
  fireEvent.click(first);
  fireEvent.click(button('Create separate library…'));
  await screen.findByText(/prepared with 1 document/);
  expect(native).toHaveBeenCalledWith('native_profile_migration', { request: {
    operation: 'prepare', operationId,
    documents: [{ documentId: documents[0]!.documentId, revision: documents[0]!.revision }],
  } });
  expect(native.mock.calls.some(([, args]) => args.request.operation === 'activate')).toBe(false);
  expect(screen.queryByRole('button', { name: 'Return to original library…' })).toBeNull();
});

test('unknown preparation keeps the selection and requires explicit close and resume before opening', async () => {
  boundary((request) => {
    if (request.operation === 'preview') return reply('preview', true);
    if (request.operation === 'prepare') return Promise.reject({ kind: 'migration_unconfirmed' });
    if (request.operation === 'close') return reply('closed');
    if (request.operation === 'resume') return reply('prepared');
    if (request.operation === 'activate') return reply('launch_requested');
    throw new Error('Unexpected native action');
  });
  render(<BackupSettings />);
  fireEvent.click(button('Import reader data…'));
  fireEvent.click(await screen.findByRole('checkbox', { name: /First original/ }));
  fireEvent.click(button('Create separate library…'));
  expect((await screen.findByRole('alert')).textContent).toContain('could not be confirmed');
  expect(button('Create separate library…').disabled).toBe(true);
  expect((screen.getByRole('checkbox', { name: /First original/ }) as HTMLInputElement).checked).toBe(true);
  expect(native.mock.calls.filter(([, args]) => args.request.operation === 'prepare')).toHaveLength(1);
  fireEvent.click(button('Done'));
  await waitFor(() => expect(button('Continue an import…').disabled).toBe(false));
  expect(document.activeElement).toBe(button('Import reader data…'));
  fireEvent.click(button('Continue an import…'));
  await screen.findByText(/prepared with 1 document/);
  expect(native).toHaveBeenCalledWith('native_profile_migration', { request: { operation: 'resume' } });
  expect(native.mock.calls.some(([, args]) => args.request.operation === 'activate')).toBe(false);
  fireEvent.click(button('Open separate library'));
  await screen.findByText(/Opening the separate library was requested/);
  expect(native).toHaveBeenCalledWith('native_profile_migration', { request: { operation: 'activate', operationId } });
  expect(screen.queryByRole('button', { name: 'Return to original library…' })).toBeNull();
});

test('only activated resume offers rollback and confirmation preserves changes before requesting the original', async () => {
  let resumes = 0;
  boundary((request) => {
    if (request.operation === 'resume') return reply(++resumes === 1 ? 'prepared' : 'activated');
    if (request.operation === 'close') return reply('closed');
    if (request.operation === 'rollback') return reply('rollback_launch_requested');
    throw new Error('Unexpected native action');
  });
  render(<BackupSettings />);
  fireEvent.click(button('Continue an import…'));
  await screen.findByText(/prepared with 1 document/);
  expect(screen.queryByRole('button', { name: 'Return to original library…' })).toBeNull();
  fireEvent.click(button('Done'));
  await waitFor(() => expect(button('Continue an import…').disabled).toBe(false));
  fireEvent.click(button('Continue an import…'));
  fireEvent.click(await screen.findByRole('button', { name: 'Return to original library…' }));
  expect(screen.getByText(/Close the separate imported app first/)).toBeTruthy();
  expect(document.activeElement).toBe(button('Keep separate library'));
  expect(native.mock.calls.some(([, args]) => args.request.operation === 'rollback')).toBe(false);
  fireEvent.click(button('Back up changes and open original library'));
  await screen.findByText(/The original library was selected again, and opening it was requested/);
  expect(native).toHaveBeenCalledWith('native_profile_migration', { request: { operation: 'rollback', operationId } });
  expect(screen.queryByRole('button', { name: 'Open separate library' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Return to original library…' })).toBeNull();
});
