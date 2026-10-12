import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { simpleAgentBackend } from './agent-setup';
import { pickAndAddLibrarySource } from './settings';
vi.mock('./settings', () => ({ pickAndAddLibrarySource: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
afterEach(() => vi.resetAllMocks());
test('project picker uses saved-folder admission, without activation, installation or MCP grants', async () => {
  vi.mocked(pickAndAddLibrarySource).mockResolvedValue({ added: true, source: {
    id: 'saved-folder', name: 'Public project', path: '/public/project', state: { kind: 'available' },
    index_state: 'not-created', active: false, restore_candidate: false,
  } });
  expect(await simpleAgentBackend.chooseProjectFolder!()).toEqual({ path: '/public/project', name: 'Public project' });
  expect(pickAndAddLibrarySource).toHaveBeenCalledExactlyOnceWith();
  expect(invoke).not.toHaveBeenCalled();
});
test('cancelled folder picker does not manufacture a project', async () => {
  vi.mocked(pickAndAddLibrarySource).mockResolvedValue({ added: false, source: null });
  expect(await simpleAgentBackend.chooseProjectFolder!()).toBeNull();
  expect(invoke).not.toHaveBeenCalled();
});
test('unavailable saved folder cannot become the setup target', async () => {
  vi.mocked(pickAndAddLibrarySource).mockResolvedValue({ added: false, source: {
    id: 'saved-folder', name: 'Public project', path: '/public/project', state: { kind: 'missing' },
    index_state: 'unknown', active: false, restore_candidate: false,
  } });
  await expect(simpleAgentBackend.chooseProjectFolder!()).rejects.toThrow('Selected folder unavailable');
  expect(invoke).not.toHaveBeenCalled();
});
test('explicit installation passes only the selected configuration to the native command', async () => {
  const selection = { agent: 'claude', scope: 'project', project_root: '/public/project' } as const;
  vi.mocked(invoke).mockResolvedValue({ installed: true });
  await simpleAgentBackend.install(selection);
  expect(invoke).toHaveBeenCalledExactlyOnceWith('agent_setup_install', { selection });
  expect(pickAndAddLibrarySource).not.toHaveBeenCalled();
});
