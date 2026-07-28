import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  MOBILE_FIXTURE_IDS,
  MOBILE_FIXTURE_BEARER,
  startMobileFixtureServer,
} from './mobile-fixture.ts';
import { MOBILE_NAVIGATION_TEXT_PATTERN } from './mobile-assertions.ts';

test('mobile navigation text accepts both Chromium and WebKit whitespace', () => {
  assert.match('LibrarySearchSourcesSettings', MOBILE_NAVIGATION_TEXT_PATTERN);
  assert.match(
    'Library\nSearch\nSources\nSettings',
    MOBILE_NAVIGATION_TEXT_PATTERN,
  );
  assert.doesNotMatch('LibrarySourcesSearchSettings', MOBILE_NAVIGATION_TEXT_PATTERN);
});

test('the isolated fixture serves the real session/entity-query contract and forces offline', async () => {
  const fixture = await startMobileFixtureServer(0);
  try {
    const refused = await fetch(`${fixture.origin}/v1/session`);
    assert.equal(refused.status, 403);

    const headers = {
      authorization: `Bearer ${MOBILE_FIXTURE_BEARER}`,
      'content-type': 'application/json',
    };
    const legacy = await fetch(`${fixture.origin}/v1/session`, {
      headers: { 'X-MDReader-Session-Reference': MOBILE_FIXTURE_BEARER },
    });
    assert.equal(legacy.status, 403);
    const session = await fetch(`${fixture.origin}/v1/session`, { headers });
    assert.equal(session.status, 200);
    assert.deepEqual(await session.json(), {
      deployment_id: MOBILE_FIXTURE_IDS.deployment,
      principal_kind: 'oidc',
      scopes: ['query'],
      workspace_ids: [MOBILE_FIXTURE_IDS.workspace],
    });

    const listing = await fetch(`${fixture.origin}/v1/entity/query`, {
      method: 'POST',
      headers,
      body: JSON.stringify({
        workspace_id: MOBILE_FIXTURE_IDS.workspace,
        entity: 'document.Document',
        select: ['path', 'title', 'size'],
      }),
    });
    assert.equal(listing.status, 200);
    const listBody = (await listing.json()) as {
      pins: {
        catalog_revision_id: string;
        physical_schema_version_ids?: string[];
        provenance_rule_version_id?: string;
      };
      rows: Array<{
        record_key: string;
        fields: { title: string };
        provenance: Record<string, { provenance_rule_version_id?: string }>;
      }>;
      sources: Array<{ source_id: string }>;
      next_cursor?: unknown;
    };
    assert.equal(listBody.pins.catalog_revision_id, MOBILE_FIXTURE_IDS.catalog);
    assert.deepEqual(listBody.pins.physical_schema_version_ids, [
      MOBILE_FIXTURE_IDS.physicalSchema,
    ]);
    assert.equal(
      listBody.pins.provenance_rule_version_id,
      MOBILE_FIXTURE_IDS.provenanceRule,
    );
    assert.equal(listBody.next_cursor, null);
    assert.equal(listBody.sources[0]?.source_id, MOBILE_FIXTURE_IDS.source);
    assert.equal(listBody.rows.length, 1);
    assert.equal(listBody.rows[0]?.record_key, 'mobile-e2e-document');
    assert.equal(listBody.rows[0]?.fields.title, 'Mobile evidence handbook');
    for (const field of Object.values(listBody.rows[0]?.provenance ?? {})) {
      assert.equal(
        field.provenance_rule_version_id,
        MOBILE_FIXTURE_IDS.provenanceRule,
      );
    }

    const readQuery = {
      workspace_id: MOBILE_FIXTURE_IDS.workspace,
      entity: 'document.Document',
      select: ['path', 'title', 'size', 'body'],
    };
    const online = await fetch(`${fixture.origin}/v1/entity/query`, {
      method: 'POST',
      headers,
      body: JSON.stringify(readQuery),
    });
    assert.equal(online.status, 200);
    assert.match(await online.text(), /Mobile evidence handbook/);

    const offline = await fetch(`${fixture.origin}/v1/entity/query`, {
      method: 'POST',
      headers,
      body: JSON.stringify(readQuery),
    });
    assert.equal(offline.status, 503);
  } finally {
    await fixture.close();
  }
});
