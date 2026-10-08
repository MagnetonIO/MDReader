import type {
  NativeProvidersBackend, ProviderLinkResolution, ProviderReaderReference,
} from '../../platform/providers';

/** Device UUIDs are explicit receipts, never inferred from a private cache path. */
export type CanonicalReaderReference = ProviderReaderReference & { documentId?: string };

/** Bytes from a native exact-document image read, never a filesystem URL. */
export interface LocalImageBytes {
  mime: 'image/png' | 'image/jpeg';
  base64: string;
  width: number;
  height: number;
}

/** Exact local candidates never carry provider handles or invented job receipts. */
export interface LocalLinkResolution {
  state: 'local_matches';
  references: ProviderReaderReference[];
}
export type CanonicalReaderBackend = Pick<NativeProvidersBackend, 'readDocument'> &
  Partial<Pick<NativeProvidersBackend, 'readCached' | 'fetchCurrent'>> & {
    readImage?: (reference: ProviderReaderReference, target: string) => Promise<LocalImageBytes>;
    resolveLink?: (
      reference: ProviderReaderReference, kind: 'relative' | 'wikilink', target: string,
    ) => Promise<ProviderLinkResolution | LocalLinkResolution>;
  };
