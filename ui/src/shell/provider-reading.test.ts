import { expect, test } from 'vitest';
import { providerTarget } from './provider-reading';
const id = `provider:${'a'.repeat(64)}`;
const projection = { path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64) };
test('canonical provider hit routes by exact revision and refuses local-path or missing-revision fallback', () => {
  expect(providerTarget({sourceId:id,path:projection.path,revision:projection.revision,name:'Plan',line:3})).toEqual({sourceId:id,path:projection.path,revision:projection.revision,title:'Plan',line:3});
  expect(()=>providerTarget({sourceId:id,path:projection.path,name:'Plan'})).toThrow(/exact revision/);
  expect(()=>providerTarget({sourceId:id,path:'/private/cache/plan.md',revision:projection.revision,name:'Plan'})).toThrow(/exact revision/);
  expect(providerTarget({sourceId:'local-source',path:'/granted/local.md',name:'Local'})).toBeNull();
});

test('device and capture search hits use exact service receipts without filesystem fallback', () => {
  const device = { deviceDocumentId: '00000000-0000-4000-8000-000000000001', sourceKind: 'device' as const,
    sourceId: 'a'.repeat(64), path: 'document.md', revision: 'b'.repeat(64), name: 'Synced', line: 9 };
  expect(providerTarget(device)).toEqual({ documentId: device.deviceDocumentId, sourceId: device.sourceId,
    path: 'document.md', revision: device.revision, title: 'Synced', line: 9 });
  expect(() => providerTarget({ ...device, deviceDocumentId: undefined })).toThrow();
  expect(() => providerTarget({ ...device, path: '/private/copy.md' })).toThrow();
  expect(() => providerTarget({ ...device, revision: undefined })).toThrow();
  const capture = { sourceKind: 'capture' as const, sourceId: 'c'.repeat(64), path: 'outputs/plan.md', revision: 'd'.repeat(64), name: 'Plan' };
  expect(providerTarget(capture)).toMatchObject({ sourceId: capture.sourceId, path: capture.path, revision: capture.revision, title: 'Plan' });
  expect(() => providerTarget({ ...capture, path: '../plan.md' })).toThrow();
  expect(() => providerTarget({ ...capture, revision: undefined })).toThrow();
});
