/**
 * A tiny keymap registry — no library. It is the single source of truth for
 * both keyboard dispatch and the shortcut sheet, so a binding and its
 * documentation can never drift. A chord is written portably with `Mod` (⌘ on
 * macOS, Ctrl elsewhere); `Ctrl` always means the literal Control key on every
 * platform (so `Ctrl+Tab` MRU cycling behaves natively on macOS).
 *
 * Modifier matching is EXACT: `Mod+K` fires only when exactly the primary
 * modifier is held, so it never also triggers under `Mod+Shift+K`.
 */

/** The minimal keyboard-event shape the registry needs (a real KeyboardEvent fits). */
export interface KeyLike {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export interface Binding {
  /** Stable id (also used as a React key in the sheet). */
  id: string;
  /** Portable chord, e.g. `Mod+Shift+T`, `Ctrl+Tab`, `Mod+.`. */
  keys: string;
  /** Human description shown in the shortcut sheet. */
  description: string;
  /** Sheet grouping, e.g. `Tabs`, `Navigation`, `View`. */
  group: string;
  /** Effect to run when the chord fires. */
  run: () => void;
  /** When true, the binding still fires while a text input is focused. */
  allowInInput?: boolean;
}

interface Chord {
  meta: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  key: string;
}

function normalizeKey(key: string): string {
  return key.length === 1 ? key.toLowerCase() : key.toLowerCase();
}

/** Parse a portable chord spec into the exact modifier state it requires. */
export function parseChord(spec: string, isMac: boolean): Chord {
  let needMod = false;
  let needCtrl = false;
  let needAlt = false;
  let needShift = false;
  let key = '';

  for (const rawPart of spec.split('+')) {
    const part = rawPart.trim().toLowerCase();
    switch (part) {
      case 'mod':
      case 'cmd':
      case 'meta':
      case 'super':
        needMod = true;
        break;
      case 'ctrl':
      case 'control':
        needCtrl = true;
        break;
      case 'alt':
      case 'opt':
      case 'option':
        needAlt = true;
        break;
      case 'shift':
        needShift = true;
        break;
      default:
        key = normalizeKey(rawPart.trim());
    }
  }

  return {
    meta: isMac ? needMod : false,
    ctrl: needCtrl || (!isMac && needMod),
    alt: needAlt,
    shift: needShift,
    key,
  };
}

function eventMatches(chord: Chord, event: KeyLike): boolean {
  return (
    event.metaKey === chord.meta &&
    event.ctrlKey === chord.ctrl &&
    event.altKey === chord.alt &&
    event.shiftKey === chord.shift &&
    normalizeKey(event.key) === chord.key
  );
}

/** Pretty-print a chord for display (⌘⇧T on macOS, Ctrl+Shift+T elsewhere). */
export function formatChord(spec: string, isMac: boolean): string {
  const parts = spec.split('+').map((raw) => raw.trim());
  const symbols: string[] = [];
  let key = '';
  for (const part of parts) {
    switch (part.toLowerCase()) {
      case 'mod':
      case 'cmd':
      case 'meta':
      case 'super':
        symbols.push(isMac ? '⌘' : 'Ctrl');
        break;
      case 'ctrl':
      case 'control':
        symbols.push(isMac ? '⌃' : 'Ctrl');
        break;
      case 'alt':
      case 'opt':
      case 'option':
        symbols.push(isMac ? '⌥' : 'Alt');
        break;
      case 'shift':
        symbols.push(isMac ? '⇧' : 'Shift');
        break;
      default:
        key = part.length === 1 ? part.toUpperCase() : part;
    }
  }
  symbols.push(key);
  return symbols.join(isMac ? '' : '+');
}

export class KeymapRegistry {
  private readonly bindings: Binding[] = [];
  private readonly chords = new Map<string, Chord>();

  constructor(private readonly isMac: boolean) {}

  register(binding: Binding): void {
    this.bindings.push(binding);
    this.chords.set(binding.id, parseChord(binding.keys, this.isMac));
  }

  registerAll(bindings: Binding[]): void {
    for (const binding of bindings) this.register(binding);
  }

  /**
   * Dispatch an event to the first matching binding. Returns true when a binding
   * ran (the caller should then `preventDefault`). Bindings are skipped while a
   * text field is focused unless they opt in with `allowInInput`.
   */
  dispatch(event: KeyLike, opts: { inEditable?: boolean } = {}): boolean {
    for (const binding of this.bindings) {
      const chord = this.chords.get(binding.id)!;
      if (!eventMatches(chord, event)) continue;
      if (opts.inEditable && !binding.allowInInput) return false;
      binding.run();
      return true;
    }
    return false;
  }

  /** All bindings in registration order (for the shortcut sheet). */
  list(): Binding[] {
    return [...this.bindings];
  }

  /** Bindings partitioned by group, preserving first-seen order. */
  groups(): { group: string; bindings: Binding[] }[] {
    const order: string[] = [];
    const byGroup = new Map<string, Binding[]>();
    for (const binding of this.bindings) {
      if (!byGroup.has(binding.group)) {
        byGroup.set(binding.group, []);
        order.push(binding.group);
      }
      byGroup.get(binding.group)!.push(binding);
    }
    return order.map((group) => ({ group, bindings: byGroup.get(group)! }));
  }

  /** Display string for a binding's chord. */
  display(binding: Binding): string {
    return formatChord(binding.keys, this.isMac);
  }
}

/** Detect a macOS-like platform from the user agent (falls back to false). */
export function detectMac(): boolean {
  if (typeof navigator === 'undefined') return false;
  const platform =
    (navigator as { userAgentData?: { platform?: string } }).userAgentData?.platform ??
    navigator.platform ??
    '';
  return /mac|iphone|ipad|ipod/i.test(platform);
}
