/** Transitional preservation adapter. Never queried by primary catalog startup. */
import { useCallback, useEffect, useRef, useState } from 'react';
import { settingsBackend, SettingsCommandError } from '../../platform/settings';
import { useTabsStore } from '../../stores/tabs';
import { useSourcesStore } from '../../stores/sources';
import { documentKey, type CloudDocumentRef } from '../../stores/types';
import type { EntityRow, LibrarySourceExecution } from '../../bindings';
import {
  continueHydratedLibrary, hydrateLibrary, hydrateLibraryDocument,
  libraryAvailability as availabilityForLibrary, physicalRecordKey, rowsForMember,
  type LibraryAvailability, type HydratedLibrary, type HydratedLibrarySelection,
} from '../../shell/library-documents';

/** Cancellation is advisory; local generations remain the selection fence. */
async function cancelQuery(request: string): Promise<void> {
  try {
    await settingsBackend.cancelCloudDocumentQuery?.(request);
  } catch {
    // A vanished service cannot make a retired response current again.
  }
}

export function useLegacyRecords(activeTabId: string | null, semanticWorkspaceGeneration: number) {
  const [enabled, setEnabled] = useState(false);
  const enabledRef = useRef(false);
  const [libraryDocument, setLibraryDocument] =
    useState<HydratedLibrarySelection | null>(null);
  const [cloudDocuments, setCloudDocuments] = useState<Record<string, HydratedLibrarySelection>>({});
  const [libraryHydration, setLibraryHydration] = useState<HydratedLibrary>({
    evidence: [],
    failures: [],
  });
  const libraryEvidence = libraryHydration.evidence;
  const hydrationRef = useRef(libraryHydration);
  hydrationRef.current = libraryHydration;
  const [librarySearch, setLibrarySearch] = useState('');
  const [libraryLoading, setLibraryLoading] = useState(false);
  const [libraryAvailability, setLibraryAvailability] = useState<LibraryAvailability>({
    kind: 'ready',
  });
  const [libraryError, setLibraryError] = useState<string | null>(null);
  const [libraryDocumentLoading, setLibraryDocumentLoading] = useState(false);
  const [libraryDocumentError, setLibraryDocumentError] = useState<string | null>(null);
  const libraryRefreshGeneration = useRef(0);
  const libraryRefreshRequest = useRef<string | null>(null);
  const libraryDocumentGeneration = useRef(0);
  const libraryDocumentRequest = useRef<string | null>(null);
  const clearLibraryDocument = useCallback(() => {
    libraryDocumentGeneration.current += 1;
    if (settingsBackend.cancelCloudDocumentQuery && libraryDocumentRequest.current) {
      void cancelQuery(libraryDocumentRequest.current);
    }
    libraryDocumentRequest.current = null;
    setLibraryDocument(null);
    setLibraryDocumentLoading(false);
    setLibraryDocumentError(null);
  }, []);

  useEffect(() => {
    clearLibraryDocument();
  }, [activeTabId, clearLibraryDocument]);
  useEffect(() => () => {
    enabledRef.current = false;
    libraryRefreshGeneration.current++;
    clearLibraryDocument();
  }, [clearLibraryDocument]);

  const refreshLibrary = useCallback(async () => {
    if (!enabledRef.current) return null;
    const generation = libraryRefreshGeneration.current + 1;
    libraryRefreshGeneration.current = generation;
    if (settingsBackend.cancelCloudDocumentQuery && libraryRefreshRequest.current) {
      await cancelQuery(libraryRefreshRequest.current);
    }
    if (!enabledRef.current || libraryRefreshGeneration.current !== generation) return null;
    setLibraryLoading(true);
    setLibraryError(null);
    setLibraryAvailability({ kind: 'syncing', sourceNames: [] });
    try {
      const requestId = `library:${globalThis.crypto.randomUUID()}`;
      libraryRefreshRequest.current = requestId;
      const hydration = await hydrateLibrary(
        settingsBackend,
        requestId,
      );
      const { evidence: answers } = hydration;
      if (libraryRefreshGeneration.current !== generation) return;
      setLibraryHydration(hydration);
      hydrationRef.current = hydration;
      setLibraryDocument((current) => {
        if (!current) return null;
        for (const evidence of answers) {
          const member = evidence.memberships.find(
            (candidate) => candidate.membership_id === current.member.membership_id,
          );
          const row = evidence.answer.rows.find(
            (candidate) => candidate.record_key === current.row.record_key,
          );
          if (member && row) return { member, row, evidence };
        }
        return null;
      });
      setLibraryAvailability(availabilityForLibrary(hydration));
      return hydration;
    } catch (error) {
      if (libraryRefreshGeneration.current === generation) {
        setLibraryHydration({ evidence: [], failures: [] });
        setLibraryDocument(null);
        const detail =
          error instanceof SettingsCommandError &&
          typeof error.detail === 'object' &&
          error.detail !== null &&
          'kind' in error.detail &&
          typeof error.detail.kind === 'string'
            ? ` (${error.detail.kind})`
            : error instanceof Error
              ? ` (${error.message})`
              : '';
        setLibraryAvailability({
          kind: 'failed',
          message: `Library sources could not be read${detail}.`,
        });
      }
    } finally {
      if (libraryRefreshGeneration.current === generation) {
        libraryRefreshRequest.current = null;
        setLibraryLoading(false);
      }
    }
  }, []);

  const loadMoreLibrary = useCallback(async (membershipId?: string) => {
    if (!enabledRef.current || !libraryHydration.continuations?.length) return;
    const generation = libraryRefreshGeneration.current + 1;
    libraryRefreshGeneration.current = generation;
    if (settingsBackend.cancelCloudDocumentQuery && libraryRefreshRequest.current) {
      await cancelQuery(libraryRefreshRequest.current);
    }
    if (libraryRefreshGeneration.current !== generation) return;
    setLibraryLoading(true);
    setLibraryError(null);
    setLibraryAvailability({ kind: 'syncing', sourceNames: [] });
    try {
      const requestId = `library:${globalThis.crypto.randomUUID()}`;
      libraryRefreshRequest.current = requestId;
      const hydration = await continueHydratedLibrary(
        settingsBackend,
        libraryHydration,
        requestId,
        membershipId,
      );
      if (libraryRefreshGeneration.current !== generation) return;
      setLibraryHydration(hydration);
      setLibraryAvailability(availabilityForLibrary(hydration));
    } catch {
      if (libraryRefreshGeneration.current === generation) {
        setLibraryHydration((current) => ({
          evidence: current.evidence,
          failures: current.failures,
        }));
        setLibraryAvailability({ kind: 'partial', sourceNames: [] });
        setLibraryError(
          'More Library documents could not be loaded. Refresh the Library.',
        );
      }
    } finally {
      if (libraryRefreshGeneration.current === generation) {
        libraryRefreshRequest.current = null;
        setLibraryLoading(false);
      }
    }
  }, [libraryHydration]);

  useEffect(() => {
    if (enabledRef.current) void refreshLibrary();
    return () => {
      libraryRefreshGeneration.current += 1;
      if (settingsBackend.cancelCloudDocumentQuery) {
        if (libraryRefreshRequest.current) {
          void cancelQuery(libraryRefreshRequest.current);
        }
      }
      libraryRefreshRequest.current = null;
    };
  // Legacy metadata is read only after an explicit migration/history action.
  // While open, refresh when the trusted workspace grant changes.
  }, [refreshLibrary, semanticWorkspaceGeneration]);

  const openLibraryDocument = useCallback(
    async (member: LibrarySourceExecution, row: EntityRow): Promise<void> => {
      if (!enabledRef.current) return;
      const evidence = hydrationRef.current.evidence.find((answer) =>
        answer.memberships.some(
          (candidate) => candidate.membership_id === member.membership_id,
        ),
      );
      if (!evidence) return;
      const generation = libraryDocumentGeneration.current + 1;
      libraryDocumentGeneration.current = generation;
      if (settingsBackend.cancelCloudDocumentQuery && libraryDocumentRequest.current) {
        await cancelQuery(libraryDocumentRequest.current);
      }
      if (!enabledRef.current || libraryDocumentGeneration.current !== generation) return;
      const requestId = `library-document:${globalThis.crypto.randomUUID()}`;
      libraryDocumentRequest.current = requestId;
      setLibraryDocument({ member, row, evidence });
      setLibraryDocumentLoading(true);
      setLibraryDocumentError(null);
      try {
        const hydratedDocument = await hydrateLibraryDocument(
          settingsBackend,
          { member, row, evidence },
          requestId,
        );
        if (libraryDocumentGeneration.current === generation) {
          if (
            (hydratedDocument.member.kind === 'google-drive' ||
              hydratedDocument.member.kind === 'dropbox') &&
            hydratedDocument.cloudDocument
          ) {
            const recordKey = physicalRecordKey(hydratedDocument.row);
            if (!recordKey) throw new Error('Cloud document lost its provider identity.');
            const ref = {
              kind: 'cloud' as const,
              sourceId: hydratedDocument.member.registry_id,
              recordKey,
              documentKey: hydratedDocument.cloudDocument.document_key,
              name: hydratedDocument.cloudDocument.title,
            };
            setCloudDocuments((current) => ({ ...current, [documentKey(ref)]: hydratedDocument }));
            useTabsStore.getState().openTab(ref, { originRoot: null });
            useSourcesStore.getState().addRecent(ref, null);
            setLibraryDocument(null);
          } else {
            setLibraryDocument(hydratedDocument);
          }
        }
      } catch {
        if (libraryDocumentGeneration.current === generation) {
          setLibraryDocumentError('This document could not be read from its Library source.');
        }
      } finally {
        if (libraryDocumentGeneration.current === generation) {
          libraryDocumentRequest.current = null;
          setLibraryDocumentLoading(false);
        }
      }
    },
    [],
  );


  const enable = useCallback(() => {
    enabledRef.current = true;
    setEnabled(true);
    void refreshLibrary();
  }, [refreshLibrary]);
  const suspend = useCallback(() => {
    enabledRef.current = false;
    setEnabled(false);
    libraryRefreshGeneration.current++;
    if (libraryRefreshRequest.current && settingsBackend.cancelCloudDocumentQuery) {
      void cancelQuery(libraryRefreshRequest.current);
    }
    libraryRefreshRequest.current = null;
    setLibraryLoading(false);
    clearLibraryDocument();
  }, [clearLibraryDocument]);
  const reset = useCallback(() => {
    libraryRefreshGeneration.current++;
    clearLibraryDocument();
    hydrationRef.current = { evidence: [], failures: [] };
    setLibraryHydration(hydrationRef.current);
    setLibrarySearch('');
  }, [clearLibraryDocument]);
  const openRecent = useCallback(async (ref: CloudDocumentRef) => {
    enabledRef.current = true;
    setEnabled(true);
    const hydration = await refreshLibrary();
    if (!hydration || !enabledRef.current) return false;
    for (const evidence of hydration.evidence) {
      const member = evidence.memberships.find((candidate) => candidate.registry_id === ref.sourceId);
      if (!member) continue;
      const row = rowsForMember(evidence, member).find((candidate) => physicalRecordKey(candidate) === ref.recordKey);
      if (row) {
        await openLibraryDocument(member, row);
        return true;
      }
    }
    return false;
  }, [refreshLibrary, openLibraryDocument]);
  return {
    enabled, enable, suspend, reset, openRecent,
    libraryDocument, cloudDocuments, setCloudDocuments,
    libraryHydration, libraryEvidence, librarySearch, setLibrarySearch,
    libraryLoading, libraryAvailability, libraryError, setLibraryError,
    libraryDocumentLoading, libraryDocumentError,
    clearLibraryDocument, refreshLibrary, loadMoreLibrary, openLibraryDocument,
  };
}
