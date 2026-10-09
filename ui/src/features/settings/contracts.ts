import type { ProviderReaderReference } from '../../platform/providers.js';
import type { ImportState } from '../../platform/fs.js';
import type { LibrarySourceDto, SettingsBackend } from '../../platform/settings.js';
import type { WatcherStatus } from '../../stores/sources.js';

import type { CanonicalReaderReference } from '../reader/contracts';
import type { DeviceReference } from '../device-reader/backend';

export type SettingsSection = 'appearance' | 'library' | 'account' | 'backup' | 'advanced';

export interface SettingsSheetProps {
  initialSection?: SettingsSection;
  returnController?: boolean;
  resumeReturnOnOpen?: boolean;
  onReturnResumeStarted?: () => void;
  selectedDevice?: DeviceReference;
  localRoot?: string | null;
  backend?: SettingsBackend;
  watcher: WatcherStatus;
  importState: ImportState | null;
  onActivateSource: (source: LibrarySourceDto) => Promise<boolean>;
  onForgetActiveSource: (source: LibrarySourceDto) => Promise<void>;
  onSourceIntent?: () => void;
  onLibraryChanged?: () => void;
  onOpenProviderDocument?: (reference: ProviderReaderReference) => Promise<void>;
  onOpenImportedDocument?: (reference: CanonicalReaderReference) => void;
  onReconnectImportedFolder?: () => Promise<void>;
  onOpenReadingLink?: (reference: ProviderReaderReference) => void | Promise<void>;
  onClose: () => void;
}
