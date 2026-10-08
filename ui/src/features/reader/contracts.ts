import type {
  NativeProvidersBackend, ProviderLinkResolution, ProviderReaderReference,
} from '../../platform/providers';

/** Device UUIDs are explicit receipts, never inferred from a private cache path. */
export type CanonicalReaderReference = ProviderReaderReference & { documentId?: string };

/** Exact local candidates never carry provider handles or invented job receipts. */
export interface LocalLinkResolution {
  state: 'local_matches';
  references: ProviderReaderReference[];
}
export type CanonicalReaderBackend = Pick<NativeProvidersBackend, 'readDocument'> &
  Partial<Pick<NativeProvidersBackend, 'readCached' | 'fetchCurrent'>> & {
    resolveLink?: (
      reference: ProviderReaderReference, kind: 'relative' | 'wikilink', target: string,
    ) => Promise<ProviderLinkResolution | LocalLinkResolution>;
  };
