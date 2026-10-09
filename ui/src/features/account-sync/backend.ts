import { commands, type AccountRequest as GeneratedAccountRequest, type AccountReply, type AccountError } from '../../bindings.js';
export type { AccountReply, AccountAction, AccountStatus, AccountJob, SelectedOperation, SelectedScope } from '../../bindings.js';
export type AccountRequest = GeneratedAccountRequest;
export interface AccountBackend { request(operation: AccountRequest): Promise<AccountReply> }
export const accountBackend: AccountBackend = {
  async request(operation) {
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
    existing_library: 'This account already has a sync library. Connect from your existing device or use recovery. The pending setup remains retained.',
    enrollment_denied: 'Enrollment was denied. Existing account custody remains retained.',
    identity_or_scope: 'Use the same sign-in account as your connected device. This sign-in did not match the selected sync library; your local library is kept.',
    reconnect_required: 'Sign in again to resume this library. Offline data remains available.',
    interrupted: 'The operation was interrupted; its remote outcome may be uncertain. Refresh state before retrying.',
    cancelled: 'The operation was cancelled. Previously durable state remains retained.',
    limit: 'This action exceeds a bounded input or account quota.',
    unavailable: 'The account service did not reply. Its outcome may be unknown; refresh state before retrying.',
  };
  return messages[kind] ?? messages.unavailable!;
}
