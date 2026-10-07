import { platform } from '@tauri-apps/plugin-os';

import { DeviceAppShell } from './mobile/DeviceAppShell';
import { AppShell } from './shell/AppShell';
import { isTauri } from './shell/persistence';

/**
 * Select the shipping composition from Tauri's compile-time platform value.
 *
 * Browser previews and unit tests without a Tauri runtime retain the desktop
 * shell. A mobile build never mounts that shell, even briefly.
 */
export default function App() {
  const runtime = isTauri() ? platform() : 'macos';
  if (runtime === 'ios' || runtime === 'android') {
    return <DeviceAppShell platform={runtime} />;
  }
  return <AppShell />;
}
