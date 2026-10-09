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
const reply = (state: ImportReply['state'], preview = false): ImportReply => ({ state, operationId, origin: 'reader', coverage: null, documents: preview ? documents : [], imported: preview ? 0 : 1 });
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

test('activated resume restores return after reopening Settings and preserves changes before requesting the original', async () => {
  let resumes = 0;
  boundary((request) => {
    if (request.operation === 'resume') return reply(++resumes === 1 ? 'prepared' : 'activated');
    if (request.operation === 'close') return reply('closed');
    if (request.operation === 'rollback') return reply('rollback_launch_requested');
    throw new Error('Unexpected native action');
  });
  const settings = render(<BackupSettings />);
  fireEvent.click(button('Continue an import…'));
  await screen.findByText(/prepared with 1 document/);
  expect(screen.queryByRole('button', { name: 'Return to original library…' })).toBeNull();
  fireEvent.click(button('Done'));
  await waitFor(() => expect(button('Continue an import…').disabled).toBe(false));
  fireEvent.click(button('Continue an import…'));
  await screen.findByRole('button', { name: 'Return to original library…' });
  await screen.findByText(/This imported library is selected/);
  settings.unmount();
  await waitFor(() => expect(native.mock.calls.filter(([, args]) => args.request.operation === 'close')).toHaveLength(2));
  const callsBeforeReopen = native.mock.calls.length;
  render(<BackupSettings />);
  expect(native.mock.calls).toHaveLength(callsBeforeReopen);
  expect(screen.queryByRole('button', { name: 'Return to original library…' })).toBeNull();
  fireEvent.click(button('Continue an import…'));
  fireEvent.click(await screen.findByRole('button', { name: 'Return to original library…' }));
  expect(screen.getByText(/close its window normally when asked/)).toBeTruthy();
  expect(document.activeElement).toBe(button('Keep separate library'));
  expect(native.mock.calls.some(([, args]) => args.request.operation === 'rollback')).toBe(false);
  fireEvent.click(button('Back up changes and open original library'));
  await screen.findByText(/The original library was selected again, and opening it was requested/);
  expect(native).toHaveBeenCalledWith('native_profile_migration', { request: { operation: 'rollback', operationId } });
  expect(native.mock.calls.filter(([, args]) => args.request.operation === 'resume')).toHaveLength(3);
  expect(native.mock.calls.some(([, args]) => ['preview', 'prepare', 'activate'].includes(args.request.operation))).toBe(false);
  expect(screen.queryByRole('button', { name: 'Open separate library' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Return to original library…' })).toBeNull();
});

test('native return controller resumes once without a picker and retries the same preservation-first return explicitly', async () => {
  let returns = 0;
  boundary((request) => {
    if (request.operation === 'resume') return reply('activated');
    if (request.operation === 'resume_return') return { ...reply('rollback_handoff_pending'), origin: 'legacy_catdb', imported: 0 };
    if (request.operation === 'close') return reply('closed');
    if (request.operation === 'rollback') return ++returns === 1
      ? { ...reply('return_controller_requested'), origin: 'legacy_catdb', imported: 0 }
      : reply('rollback_launch_requested');
    throw new Error('No picker, preparation or activation during return');
  });
  const imported = render(<BackupSettings />);
  fireEvent.click(button('Continue an import…'));
  fireEvent.click(await screen.findByRole('button', { name: 'Return to original library…' }));
  fireEvent.click(button('Back up changes and open original library'));
  await screen.findByText(/Return controls were requested in a separate window/);
  expect(screen.queryByRole('button', { name: 'Retry return to original library' })).toBeNull();
  imported.unmount();
  await waitFor(() => expect(native.mock.calls.some(([, args]) => args.request.operation === 'close')).toBe(true));

  const started = vi.fn();
  const controller = render(<BackupSettings returnController resumeReturnOnOpen onReturnResumeStarted={started} />);
  await screen.findByRole('heading', { name: 'Close the imported MDReader window' });
  expect(started).toHaveBeenCalledTimes(1);
  expect(native.mock.calls.filter(([, args]) => args.request.operation === 'resume_return')).toHaveLength(1);
  expect(returns).toBe(1);
  controller.rerender(<BackupSettings returnController resumeReturnOnOpen={false} onReturnResumeStarted={started} />);
  expect(screen.queryByRole('button', { name: 'Import reader data…' })).toBeNull();
  fireEvent.click(button('Retry return to original library'));
  await screen.findByText(/The original library was selected again, and opening it was requested/);
  const rollbacks = native.mock.calls.filter(([, args]) => args.request.operation === 'rollback');
  expect(rollbacks).toHaveLength(2);
  for (const [, args] of rollbacks) expect(args.request).toEqual({ operation: 'rollback', operationId });
  expect(native.mock.calls.some(([, args]) => ['preview', 'prepare', 'activate', 'retry'].includes(args.request.operation))).toBe(false);
});

test.each([
  ['background_service_unsupported', 'The original background service cannot finish the handoff'],
  ['service_handoff_unconfirmed', 'The original library handoff is not confirmed'],
] as const)('retains the selected import after %s without repeating close-window advice', async (kind, heading) => {
  boundary((request) => {
    if (request.operation === 'preview') return {
      ...reply('close_original_required'), origin: 'legacy_catdb', imported: 0,
    };
    if (request.operation === 'retry') return Promise.reject({ kind });
    if (request.operation === 'close') return reply('closed');
    throw new Error('Unexpected native action');
  });
  render(<BackupSettings />);
  fireEvent.click(button('Import reader data…'));
  await screen.findByRole('heading', { name: 'Close the original MDReader app' });
  fireEvent.click(button('Retry import'));
  await screen.findByRole('heading', { name: heading });
  expect(screen.queryByRole('heading', { name: 'Close the original MDReader app' })).toBeNull();
  expect(screen.getByRole('alert').textContent).toContain('original');
  const retries = () => native.mock.calls.filter(([, args]) => args.request.operation === 'retry');
  expect(retries()).toHaveLength(1);
  await waitFor(() => expect(button('Retry import').disabled).toBe(false));
  fireEvent.click(button('Retry import'));
  await waitFor(() => expect(retries()).toHaveLength(2));
  for (const [, args] of retries()) expect(args.request).toEqual({ operation: 'retry', operationId });
  expect(native.mock.calls.filter(([, args]) => args.request.operation === 'preview')).toHaveLength(1);
  expect(native.mock.calls.some(([, args]) => ['prepare', 'activate', 'resume'].includes(args.request.operation))).toBe(false);
  await waitFor(() => expect(button('Retry import').disabled).toBe(false));
  expect(screen.queryByRole('heading', { name: 'Close the original MDReader app' })).toBeNull();
});

test.each([
  ['background_service_unsupported', 'The original background service cannot finish the handoff'],
  ['service_handoff_pending', 'The original library handoff is not confirmed'],
] as const)('keeps a reader-origin %s operation visible when native refuses dismissal', async (state, heading) => {
  boundary((request) => {
    if (request.operation === 'preview') return { ...reply(state), imported: 0 };
    if (request.operation === 'close') return Promise.reject({ kind: 'service_handoff_unconfirmed' });
    if (request.operation === 'retry') return reply('preview', true);
    throw new Error('Unexpected native action');
  });
  render(<BackupSettings />);
  fireEvent.click(button('Import reader data…'));
  await screen.findByRole('heading', { name: heading });
  expect(screen.queryByRole('heading', { name: 'Close the original MDReader app' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Create separate library…' })).toBeNull();
  expect(native.mock.calls.some(([, args]) => args.request.operation === 'retry')).toBe(false);
  fireEvent.click(button('Done'));
  expect((await screen.findByRole('alert')).textContent).toContain('cannot be dismissed yet');
  await waitFor(() => expect(button('Retry import').disabled).toBe(false));
  expect(button('Import reader data…').disabled).toBe(true);
  fireEvent.click(button('Retry import'));
  await screen.findByRole('checkbox', { name: /First original/ });
  expect(native).toHaveBeenCalledWith('native_profile_migration', { request: { operation: 'retry', operationId } });
  expect(native.mock.calls.filter(([, args]) => args.request.operation === 'preview')).toHaveLength(1);
  expect(native.mock.calls.filter(([, args]) => args.request.operation === 'retry')).toHaveLength(1);
});

test.each(['activated', 'rollback_handoff_pending'] as const)('retries only the same return operation after %s handoff uncertainty', async (state) => {
  let attempts = 0;
  boundary((request) => {
    if (request.operation === 'resume') return { ...reply(state), imported: state === 'activated' ? 1 : 0 };
    if (request.operation === 'rollback') {
      if (++attempts === 1) return Promise.reject({ kind: 'service_handoff_unconfirmed' });
      return reply('rollback_launch_requested');
    }
    if (request.operation === 'close') return Promise.reject({ kind: 'service_handoff_unconfirmed' });
    throw new Error('Unexpected native action');
  });
  render(<BackupSettings />);
  fireEvent.click(button('Continue an import…'));
  if (state === 'activated') {
    fireEvent.click(await screen.findByRole('button', { name: 'Return to original library…' }));
    fireEvent.click(button('Back up changes and open original library'));
  } else {
    fireEvent.click(await screen.findByRole('button', { name: 'Retry return to original library' }));
  }
  await screen.findByRole('alert');
  await waitFor(() => expect(button('Retry return to original library').disabled).toBe(false));
  expect(screen.queryByRole('button', { name: 'Retry import' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Create separate library…' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Open separate library' })).toBeNull();
  fireEvent.click(button('Retry return to original library'));
  await screen.findByText(/The original library was selected again, and opening it was requested/);
  const returns = native.mock.calls.filter(([, args]) => args.request.operation === 'rollback');
  expect(returns).toHaveLength(2);
  for (const [, args] of returns) expect(args.request).toEqual({ operation: 'rollback', operationId });
  expect(native.mock.calls.some(([, args]) => ['preview', 'retry', 'prepare', 'activate'].includes(args.request.operation))).toBe(false);
});
