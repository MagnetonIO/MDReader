export interface AndroidArchiveScanLimits {
  maximumNative: number;
  maximumAggregateNative: number;
  maximumLibraries: number;
}

export interface AndroidArchiveDexFile {
  name: string;
  declaredSize: number;
  streamedBytes: number;
}

export interface AndroidArchiveNativeLibrary {
  name: string;
  declaredSize: number;
  streamedBytes: number;
  embeddedManifest: Buffer | null;
}

export interface AndroidArchiveScan {
  applicationId: string;
  manifestStreamedBytes: number;
  aggregateStreamedBytes: number;
  dexFiles: AndroidArchiveDexFile[];
  nativeLibraries: AndroidArchiveNativeLibrary[];
}

export function scanAndroidArchive(
  apk: string,
  limits: AndroidArchiveScanLimits,
): Promise<AndroidArchiveScan>;
