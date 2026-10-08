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
