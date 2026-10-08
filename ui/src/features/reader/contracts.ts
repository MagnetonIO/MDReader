import type {
  NativeProvidersBackend, ProviderLinkResolution, ProviderReaderReference,
} from '../../platform/providers';

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
