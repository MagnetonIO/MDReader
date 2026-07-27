export type Platform =
  | 'linux'
  | 'macos'
  | 'ios'
  | 'freebsd'
  | 'dragonfly'
  | 'netbsd'
  | 'openbsd'
  | 'solaris'
  | 'android'
  | 'windows';

let current: Platform = 'macos';

/** Test-only mirror of the compile-time value exposed by `@tauri-apps/plugin-os`. */
export function platform(): Platform {
  return current;
}

export function setTestPlatform(value: Platform): void {
  current = value;
}
