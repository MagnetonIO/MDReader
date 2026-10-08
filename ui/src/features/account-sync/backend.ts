import { invoke } from '@tauri-apps/api/core';
import { commands, type AccountRequest as GeneratedAccountRequest, type AccountReply, type AccountError, type SelectedOperation } from '../../bindings.js';
export type { AccountReply, AccountAction, AccountStatus, AccountJob, SelectedOperation, SelectedScope } from '../../bindings.js';
export type AccountRequest = GeneratedAccountRequest
  | { operation: 'sync_selected_state' | 'inspect_selected_state' | 'sync_selected_delete' | 'inspect_selected_delete'; selection: SelectedOperation };
export interface AccountBackend { request(operation: AccountRequest): Promise<AccountReply> }
export const accountBackend: AccountBackend = {
  async request(operation) {
    if (operation.operation === 'sync_selected_state' || operation.operation === 'inspect_selected_state' || operation.operation === 'sync_selected_delete' || operation.operation === 'inspect_selected_delete') {
      return invoke<AccountReply>('reader_account', { operation });
    }
    const result = await commands.readerAccount(operation);
    if (result.status === 'error') throw result.error;
    return result.data;
  },
};
export function accountError(cause: unknown): string {
  const kind = cause && typeof cause === 'object' && 'kind' in cause ? (cause as AccountError).kind : 'unavailable';
  const messages: Record<string, string> = {
    unconfigured: 'Encrypted sync is not configured in this build. Local reading remains available.',
    unsupported: 'This account action is unavailable on this platform.',
    busy: 'Another account operation is still running. Refresh its status before retrying.',
    invalid: 'Check the library ID or public device packet. The action was not accepted.',
    schema: 'This profile has an unsupported account schema. Nothing was reset.',
    storage: 'The account owner could not preserve local state. Existing data remains retained.',
    conflict: 'The account selection changed. Refresh before choosing another action.',
    custody_unavailable: 'Native secure storage is unavailable or needs attention. No new keys were substituted.',
    enrollment_denied: 'Enrollment was denied. Existing account custody remains retained.',
    identity_or_scope: 'The signed-in account or library scope did not match the selected authority.',
    reconnect_required: 'Sign in again to resume this library. Offline data remains available.',
    interrupted: 'The operation was interrupted; its remote outcome may be uncertain. Refresh state before retrying.',
    cancelled: 'The operation was cancelled. Previously durable state remains retained.',
    limit: 'This action exceeds a bounded input or account quota.',
    unavailable: 'The account service did not reply. Its outcome may be unknown; refresh state before retrying.',
  };
  return messages[kind] ?? messages.unavailable!;
}
