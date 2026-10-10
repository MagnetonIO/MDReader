import type {
  NativeProvidersBackend, ProviderLinkResolution, ProviderReaderReference,
} from '../../platform/providers';
import type { AssetPreview } from '../../bindings';

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
  linkKind?: 'relative' | 'wikilink';
}
/** Imported relationship candidates are native-verified device receipts. */
export interface DeviceLinkResolution {
  state: 'device_matches';
  references: (CanonicalReaderReference & { documentId: string })[];
}
export type CanonicalReaderBackend = Pick<NativeProvidersBackend, 'readDocument'> &
  Partial<Pick<NativeProvidersBackend, 'readCached' | 'fetchCurrent'>> & {
    /** Only known transient read failures offer explicit exact-reference retry. */
    isReadRetryable?: (cause: unknown) => boolean;
    readImage?: (reference: ProviderReaderReference, target: string) => Promise<LocalImageBytes>;
    readAttachment?: (reference: ProviderReaderReference, target: string) => Promise<AssetPreview>;
    resolveLink?: (
      reference: ProviderReaderReference, kind: 'relative' | 'wikilink', target: string,
    ) => Promise<ProviderLinkResolution | LocalLinkResolution | DeviceLinkResolution>;
  };
