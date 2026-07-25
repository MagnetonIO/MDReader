/**
 * Milestone PT — the UNIFIED harness's platform-driver contract.
 *
 * The re-architecture puts ONE runner (`runner.ts`) over ONE shared behavioural
 * spec (`../flows.ts`), driving every target — macOS, Windows, Linux, iOS,
 * Android — through a thin per-platform ADAPTER that implements this interface.
 * The adapter is where the two execution CONTEXTS live:
 *
 *   - the NATIVE context — the OS window shell: traffic-lights / close button,
 *     titlebar drag, resize, the app process itself. Driven through a real
 *     WebDriver session (Appium: mac2 / windows / xcuitest / uiautomator2, or
 *     tauri-driver's webview session where the OS chrome isn't reachable).
 *   - the WEBVIEW context — the HTML UI. On every platform whose webview exposes
 *     a WebDriver bridge (WebKitGTK, WebView2, the mobile webview contexts) the
 *     adapter reads the live DOM. On macOS, WKWebView exposes NO WebDriver bridge
 *     (industry-wide), so the mac2 adapter reads WebKit's Accessibility tree
 *     instead — WebKit publishes each element's CSS class list on `AXDOMClassList`
 *     — giving the SAME class-based semantic contract off the same `flows.ts`.
 *
 * The runner only ever calls these methods; it never knows which transport a
 * given leg uses. That is what lets one `FLOW` describe "working" identically
 * across desktop and mobile.
 *
 * FAIL-CLOSED discipline is part of the contract: every webview read
 * (`countClass`, `statusText`, `headings`, `readerText`, `paletteContainerCount`)
 * MUST throw on a genuine read error rather than return a false 0 / empty string.
 * A broken read must never masquerade as "not found / count 0", or an absence
 * assertion could false-pass. Only a successful read that genuinely found nothing
 * returns 0 / "". For the COUNTS that is stricter than "no AX/WebDriver error
 * occurred": a driver must not return 0 from a read path that could not have
 * counted anything in the first place (on macOS: an app publishing no web area,
 * or no element classes at all — see `ax-helper.ts`), because `jumpToHit` reads
 * exactly that 0 as "the palette is gone".
 */

import type { Chord } from '../flows.ts';

/** A window rectangle in global, top-left-origin screen coordinates. */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** Where a synthetic drag grabs the window. The titlebar is the only legitimate
 *  drag surface; a content-area drag is the NEGATIVE CONTROL that must NOT move
 *  the window. */
export type DragOrigin = 'titlebar' | 'content';

/** An opaque before-image handle produced by {@link PlatformDriver.captureFrame}
 *  and consumed by {@link PlatformDriver.assertFrameChanged}. */
export type FrameToken = Buffer;

/**
 * The active fault id, from `MDREADER_SMOKE_FAULT`. Empty on normal runs. The
 * runner applies the BEHAVIOURAL faults (break what a step exercises); a driver
 * applies its own READ-PATH faults (break the harness's read of the result) —
 * see `mac2-adapter.ts`. Every fault must turn the targeted step RED, never be
 * read as a false pass. Shared here so both sides read the same variable.
 */
export const FAULT: string = process.env.MDREADER_SMOKE_FAULT ?? '';
export const faulted = (id: string): boolean => FAULT === id;

/**
 * The primitives the unified runner drives every platform through. A driver that
 * cannot rigorously assert the native-window steps (a webview-only WebDriver
 * session) sets {@link windowOps} to false and gives a {@link windowPendingReason};
 * the runner then registers those steps PENDING with that reason instead of
 * silently skipping — the coverage boundary stays explicit.
 */
export interface PlatformDriver {
  /** Human label for logs, e.g. "macOS (appium-mac2-driver)". */
  readonly name: string;
  /** Can this driver drive AND rigorously assert the native-window steps
   *  (drag / resize / red-X close)? True for OS-chrome-capable Appium drivers
   *  (mac2 today; windows/xcuitest/uiautomator2 where they run); false for a
   *  webview-only session (tauri-driver), which marks them pending. */
  readonly windowOps: boolean;
  /** Why the window steps are pending, when {@link windowOps} is false. */
  readonly windowPendingReason?: string;
  /** Does this driver back its webview interaction steps with a before/after
   *  frame-diff visual oracle? Only the macOS leg does (screencapture); the
   *  WebDriver DOM legs assert the DOM directly and need no pixel corroboration. */
  readonly frameOracle: boolean;
  /** Can this driver drive AND rigorously assert the P7 semantic surfaces
   *  (View As, Semantic Inspector, Constraint Explorer, Query Explanation)?
   *  They need two primitives beyond the class counts the base contract has:
   *  clicking a specific in-webview control located by what it SAYS
   *  ({@link clickIn}) and reading the text under one class
   *  ({@link scopedText}). True on the macOS mac2 leg (AX frames +
   *  `macos: click`); false elsewhere until that leg is exercised, and then the
   *  runner registers those steps PENDING with {@link semanticPendingReason}
   *  rather than skipping them silently. */
  readonly semanticOps: boolean;
  /** Why the semantic steps are pending, when {@link semanticOps} is false. */
  readonly semanticPendingReason?: string;

  /** Bring the app to front / make its window key — the precondition for both
   *  keystrokes to land and (on macOS) an unoccluded capture. */
  focus(): Promise<void>;
  /** Tear down the driver's session + any process it owns. Best-effort. */
  dispose(): Promise<void>;

  // ---- WEBVIEW context: semantic reads (all FAIL CLOSED) ----

  /** True once the app shell has mounted (used by the `launch` step for
   *  webview-only drivers, which have no native-window signal to assert). */
  shellMounted(): Promise<boolean>;
  /** Count elements whose CSS class list contains `cssClass` (DOM `.class`
   *  selector on WebDriver legs; `AXDOMClassList` on macOS). */
  countClass(cssClass: string): Promise<number>;
  /** Text of the status-bar region (carries "Indexed N files"). */
  statusText(): Promise<string>;
  /** Text of every rendered heading. */
  headings(): Promise<string>;
  /** Text scoped to the reader region (so the sidebar/status bar can't supply a
   *  false heading/provenance match). */
  readerText(): Promise<string>;
  /** Count of the search palette's own container — used for POSITIVE closure in
   *  `jumpToHit` (closure is asserted directly, never inferred from a result-row
   *  count of 0). Split out from {@link countClass} so a driver can inject its
   *  AX-read fault probes here without touching the other reads. */
  paletteContainerCount(): Promise<number>;

  // ---- input ----

  /** Send a shell chord (`Mod+P`, `Mod+K`, `Enter`, `Escape`, `Mod+Shift+…`)
   *  to the focused UI, the way the platform's own automation types. */
  chord(chord: Chord): Promise<void>;
  /**
   * Deliver the same chord STRAIGHT to the app under test, bypassing any other
   * application's claim on the combination.
   *
   * On macOS any app can register a chord as a global hot key, and the window
   * server then consumes it before the frontmost app is offered it — so a
   * shortcut the product implements correctly can produce nothing at all on a
   * particular machine. The runner uses this ONLY as a second attempt, after
   * {@link chord} has been tried and demonstrably opened nothing, and it says
   * in the step's own result when it had to. Never a substitute for the real
   * typing path: it is what tells "this machine ate the keystroke" apart from
   * "the app ignored it", and both must stay distinguishable.
   */
  chordDirect?(chord: Chord): Promise<void>;
  /** Type literal text into the focused input. */
  type(text: string): Promise<void>;

  // ---- P7 semantic surfaces (only meaningful when semanticOps === true) ----

  /** Text under every element carrying `cssClass`. Same fail-closed rule as the
   *  other reads: a broken read THROWS, only a successful empty one returns "". */
  scopedText?(cssClass: string): Promise<string>;
  /** Click the first element carrying `cssClass` whose own text contains
   *  `contains` (when given), bringing it into view first if the surface it
   *  lives on has it scrolled out. MUST throw when there is no such element, and
   *  when it cannot be made genuinely clickable — a control that is not there,
   *  or not where it is laid out, is never a click at an estimated point. (On
   *  macOS that second case is not hypothetical: WebKit reports the frame of a
   *  control scrolled below a modal's fold, and clicking it hits the backdrop
   *  and dismisses the modal.) */
  clickIn?(cssClass: string, contains?: string): Promise<void>;
  /** Double-click the rendered text `text` in the document, selecting a word —
   *  the genuine text selection the Semantic Inspector reads a passage from (a
   *  bare click yields no selection and no claims). The passage is addressed by
   *  the words it says: the reading column's block wrappers are generic `<div>`s
   *  that WebKit leaves out of the AX tree, so no class-based locator can reach
   *  them on the macOS leg. Throws when the text is not rendered. */
  doubleClickText?(text: string): Promise<void>;

  // ---- NATIVE window context (only meaningful when windowOps === true) ----

  /** Is a native window present (the process is alive and showing UI)? */
  isWindowPresent(): Promise<boolean>;
  /** The current native window rectangle. */
  windowRect(): Promise<Rect>;
  /** Is the app process still alive? */
  isProcessAlive(): Promise<boolean>;
  /** One synthetic drag grabbing at `origin` and pulling by (dx,dy). */
  dragBy(origin: DragOrigin, dx: number, dy: number): Promise<void>;
  /** Resize the window by (dw,dh) via a native gesture; returns the new rect. */
  resizeBy(dw: number, dh: number): Promise<Rect>;
  /** Click the native red close button. */
  clickClose(): Promise<void>;

  // ---- frame oracle (present only when frameOracle === true) ----

  /** Focus + raise the window and grab its current frame as the BEFORE image. */
  captureFrame?(): Promise<FrameToken>;
  /** Grab the AFTER frame, diff against `before`, and assert a real visible
   *  change; returns the change fraction. Retries a capture anomaly a bounded
   *  number of times, then FAILS — never accepts a blank/wrong-size grab as
   *  "changed". */
  assertFrameChanged?(before: FrameToken, label: string): Promise<number>;
}

/** The outcome of one flow step. */
export interface StepResult {
  readonly id: string;
  readonly title: string;
  readonly channel: 'dom' | 'window';
  readonly status: 'pass' | 'fail' | 'pending';
  readonly detail: string;
  readonly ms: number;
}

/** The outcome of a whole flow run. */
export interface FlowResult {
  readonly ok: boolean;
  readonly failedAt: string | null;
  readonly results: readonly StepResult[];
}
