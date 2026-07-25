/**
 * The semantic preferences the shell persists: the reviewer a review is signed
 * with, the lens in force, and the Inspector rail's state.
 *
 * The reviewer is the one that carries a guarantee rather than a convenience.
 * The app has no identity of its own and will not invent one, so "no reviewer"
 * has to be representable and has to be distinguishable from a name made of
 * spaces — the store is where that distinction is enforced, before anything can
 * reach a backend that (rightly) refuses a blank.
 */

import { afterEach, describe, expect, test } from 'vitest';

import { hasReviewer, useSettingsStore } from './settings.js';

afterEach(() => useSettingsStore.getState().reset());

describe('the reviewer identity', () => {
  test('starts unset — there is no default reviewer and no placeholder', () => {
    const state = useSettingsStore.getState();
    expect(state.reviewer).toBe('');
    expect(hasReviewer(state)).toBe(false);
  });

  test('a whitespace-only name is not an identity', () => {
    useSettingsStore.getState().setReviewer('   \t \n ');
    expect(useSettingsStore.getState().reviewer).toBe('');
    expect(hasReviewer(useSettingsStore.getState())).toBe(false);
  });

  test('a real name is stored trimmed, so nothing padded is ever submitted', () => {
    useSettingsStore.getState().setReviewer('  Ada Lovelace  ');
    expect(useSettingsStore.getState().reviewer).toBe('Ada Lovelace');
    expect(hasReviewer(useSettingsStore.getState())).toBe(true);
  });

  test('clearing the name returns to unset rather than to a remembered default', () => {
    useSettingsStore.getState().setReviewer('Ada');
    useSettingsStore.getState().setReviewer('');
    expect(hasReviewer(useSettingsStore.getState())).toBe(false);
  });
});

describe('the lens in force', () => {
  test('defaults to the plain reading', () => {
    expect(useSettingsStore.getState().lensSchema).toBeNull();
  });

  test('holds only a schema name, and can always be cleared', () => {
    useSettingsStore.getState().setLensSchema('decision_overlay');
    expect(useSettingsStore.getState().lensSchema).toBe('decision_overlay');
    useSettingsStore.getState().setLensSchema(null);
    expect(useSettingsStore.getState().lensSchema).toBeNull();
  });
});

describe('the inspector rail', () => {
  test('is collapsed by default — reading is the product', () => {
    expect(useSettingsStore.getState().inspectorOpen).toBe(false);
  });

  test('toggles independently of the related rail', () => {
    useSettingsStore.getState().toggleInspector();
    expect(useSettingsStore.getState().inspectorOpen).toBe(true);
    expect(useSettingsStore.getState().relatedRailOpen).toBe(false);
  });
});
