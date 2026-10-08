import type { ProviderReaderReference } from '../../platform/providers.js';
import type { ImportState } from '../../platform/fs.js';
import type { LibrarySourceDto, SettingsBackend } from '../../platform/settings.js';
import type { WatcherStatus } from '../../stores/sources.js';

import type { DeviceReference } from '../device-reader/backend';

export interface SettingsSheetProps {
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
  onClose: () => void;
}
