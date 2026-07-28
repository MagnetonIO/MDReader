/**
 * Milestone PT — the SHARED behavioural spec for MDReader's GUI, desktop + mobile.
 *
 * This is the single source of truth for the core end-to-end flow, expressed as
 * an ordered, platform-agnostic list of {@link Step}s. The UNIFIED harness runs it
 * ONCE per target through one interpreter (`drivers/runner.ts`) over a thin per-
 * platform adapter, so every leg describes the exact same "working":
 *
 *   - macOS   → `drivers/macos/mac2-adapter.ts` (appium-mac2-driver / XCUITest).
 *               Drives the native shell over the WebDriver protocol; reads
 *               WKWebView's Accessibility tree (`AXDOMClassList`) because WKWebView
 *               exposes no WebDriver bridge. RUNS locally (`runners/macos.ts`).
 *   - Linux/Windows → `drivers/webview-adapter.ts` over `tauri-driver`
 *               (WebKitGTK / WebView2) — the live webview DOM.
 *   - iOS/Android   → `drivers/webview-adapter.ts` over appium-xcuitest-driver /
 *               appium-uiautomator2-driver, in the WEBVIEW context (blocked on the
 *               P6 mobile shell; scaffolded + CI-wired).
 *
 * Each step carries its own semantics (`kind`) plus the data the interpreter
 * needs. A step's `channel` says which context it exercises:
 *
 *   - `'dom'`    — a webview interaction/assertion, asserted rigorously against the
 *                  live DOM (WebDriver legs) or WebKit's AX tree (macOS). Same
 *                  class-based contract on every leg.
 *   - `'window'` — a NATIVE-window interaction/assertion (drag / resize / red-X
 *                  close). The steps a "process booted" check can never cover — the
 *                  dead-titlebar class of bug. The macOS mac2 leg asserts these
 *                  rigorously (window position/size, process liveness); the webview
 *                  legs register them pending because a WebDriver session drives the
 *                  webview document, not the OS chrome.
 *
 * Keep this module free of any runtime dependency (Node built-ins only are used
 * by its consumers, never here) so it stays importable from both a plain
 * `node --experimental-strip-types` process and a bundler/ts-node context, and
 * JSON-serialisable (regexes are carried as pattern strings, compiled by the
 * consumer) so it can be emitted as data if a third consumer ever needs it.
 */

/** How a consumer locates an element in the webview DOM. Only the fields a given
 *  strategy needs are set; `description` is always present for human-readable
 *  logs and assertion messages. */
export interface DomSelector {
  /** A raw CSS selector (used as-is by WebdriverIO's `$`). */
  readonly css?: string;
  /** An ARIA role, optionally narrowed by {@link name}. */
  readonly role?: string;
  /** The exact accessible name to pair with {@link role}. */
  readonly name?: string;
  /** A regex *source* string matched against an element's text content. */
  readonly textPattern?: string;
  /** Human label, surfaced in logs and failure messages. */
  readonly description: string;
}

/** The named selectors the flow refers to. Centralised so the two interpreters
 *  — and any future one — never drift on how an element is found. Every value
 *  here is grounded in the real shipping DOM (see `apps/mdreader/ui/src/shell`
 *  and `packages/ui/src`). */
export const SELECTORS = {
  /** The sidebar file tree — proof the seeded workspace was granted + listed. */
  workspaceTree: {
    css: '[role="tree"][aria-label="Workspace files"]',
    role: 'tree',
    name: 'Workspace files',
    description: 'workspace file tree',
  },
  /** Any file row in the tree. */
  treeItem: {
    css: '[role="treeitem"]',
    role: 'treeitem',
    description: 'a workspace file row',
  },
  /** The bottom status bar — carries the "Indexed N files" index indicator. */
  statusBar: {
    css: 'footer[aria-label="Status"]',
    role: 'contentinfo',
    name: 'Status',
    description: 'status bar',
  },
  /** The search / file-jump palette overlay. */
  paletteDialog: {
    css: '[role="dialog"][aria-modal="true"]',
    role: 'dialog',
    description: 'command palette dialog',
  },
  /** The palette's text input (cmdk renders it with role=combobox). */
  paletteInput: {
    css: '.mdsearch__input',
    role: 'combobox',
    description: 'palette search input',
  },
  /** A palette result row (search hit or file suggestion). */
  paletteOption: {
    css: '[role="option"]',
    role: 'option',
    description: 'a palette result',
  },
  /** The reader's scroll container — present once a document has rendered. */
  reader: {
    css: '.mdreader',
    description: 'the reading pane',
  },
  /** A rendered top-level block inside the reader. */
  readerBlock: {
    css: '.mdreader-block',
    description: 'a rendered document block',
  },
  /** The pre-workspace welcome placeholder. */
  readerEmpty: {
    css: '.reader-empty__title',
    textPattern: 'A calm place to read your markdown',
    description: 'the welcome placeholder',
  },

  // ---- P7 W5: the five semantic surfaces (the "showcase" UI) ----

  /** The in-column lens band — present only while an interpretation is in force. */
  lensBand: {
    css: '.lens-band',
    role: 'group',
    name: 'Active lens',
    description: 'the active-lens band',
  },
  /** The Semantic view sheet. NOTE: the lens sheets set `role=dialog` + `aria-label`
   *  but NOT `aria-modal`, so {@link paletteDialog} deliberately does not match
   *  them. */
  lensSheet: {
    css: '[role="dialog"][aria-label="Semantic view"]',
    role: 'dialog',
    name: 'Semantic view',
    description: 'the Semantic view sheet',
  },
  /** One schema row inside the Semantic view sheet. */
  lensChoice: {
    css: '.lens-choice',
    role: 'button',
    description: 'a registered-schema row',
  },
  /** The expanded Semantic Inspector rail. */
  inspectorRail: {
    css: '.lens-rail',
    role: 'complementary',
    name: 'Semantic inspector',
    description: 'the semantic inspector rail',
  },
  /** One reviewable claim in the Inspector. */
  claim: {
    css: '.claim',
    description: 'an inspected claim',
  },
  /** The Constraint Explorer sheet. */
  constraintSheet: {
    css: '.lens-sheet--laws',
    role: 'dialog',
    name: 'Constraint explorer',
    description: 'the constraint explorer',
  },
  /** A click-to-source button on a located violation endpoint. */
  violationJump: {
    css: '.located__jump',
    role: 'button',
    description: 'a violation click-to-source button',
  },
  /** The Query Explanation sheet. */
  explainSheet: {
    css: '.lens-sheet--explain',
    role: 'dialog',
    name: 'Query explanation',
    description: 'the query explanation',
  },
  /** The Query Explanation's query field. */
  explainInput: {
    css: '.explain__input',
    role: 'searchbox',
    name: 'Query to explain',
    description: 'the explain query field',
  },
  /** One explained hit. */
  explainedHit: {
    css: '.explain',
    description: 'an explained search hit',
  },
  /** A rendered block the reader currently marks as the located range. THE
   *  end-of-chain oracle for click-to-source: the pulse itself is an inline
   *  background no class-based reader can see, so `markdown-reader` also stamps
   *  this class on every block the range covers. A jump whose end line was
   *  dropped marks the first block only. */
  locatedBlock: {
    css: '.mdreader-block--located',
    description: 'a block inside the located source range',
  },
} as const satisfies Record<string, DomSelector>;

export type SelectorName = keyof typeof SELECTORS;

/**
 * A keyboard chord in the shell's own notation. `Mod` is the primary modifier —
 * Cmd on macOS, Ctrl elsewhere — matching how {@link detectMac} drives the app.
 * Consumers translate it to their platform (System Events `command down` vs.
 * WebdriverIO `['Meta', 'k']`).
 */
export type Chord =
  | 'Mod+P'
  | 'Mod+K'
  | 'Enter'
  | 'Escape'
  /** Semantic view… — opens the lens sheet. */
  | 'Mod+Shift+V'
  /** Toggle the semantic inspector rail. */
  | 'Mod+Shift+I'
  /** Where the laws break — the Constraint Explorer. */
  | 'Mod+Shift+L'
  /** Explain a search — the Query Explanation. */
  | 'Mod+Shift+K';

/** Which layer a step exercises — see the module doc-comment. */
export type Channel = 'dom' | 'window';

/**
 * One ordered step of the core flow. The discriminated `kind` gives each
 * interpreter a single `switch` to act on; the extra fields carry the data that
 * step needs.
 */
export type Step =
  | {
      readonly kind: 'launch';
      readonly id: string;
      readonly title: string;
      readonly channel: 'window';
    }
  | {
      /** Assert the seeded workspace was granted: the file tree is populated. */
      readonly kind: 'grantWorkspace';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** Minimum number of tree rows expected (the fixture corpus is larger). */
      readonly minFiles: number;
    }
  | {
      /** Wait for the index to settle and assert the "Indexed N files" label. */
      readonly kind: 'waitIndexed';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** Regex source matched against the status bar text. */
      readonly pattern: string;
    }
  | {
      /** Open a document by name via the Cmd+P file palette; assert it renders. */
      readonly kind: 'openDoc';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** Query typed into the file palette to locate the doc. */
      readonly query: string;
      /** Regex source the rendered document's heading must match. */
      readonly headingPattern: string;
    }
  | {
      /** Cmd+K search for `query`; assert at least one hit comes back. */
      readonly kind: 'search';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      readonly query: string;
    }
  | {
      /** Press Enter on the top hit; assert the reader actually navigated to the
       *  hit's document AND scrolled its provenance target into view. Both legs
       *  assert this: the palette closes, a rendered heading matches
       *  {@link headingPattern}, and the provenance line ({@link provenancePattern})
       *  — text unique to the target document — is present in the reader. */
      readonly kind: 'jumpToHit';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** Regex source the jumped-to document's heading must match. */
      readonly headingPattern: string;
      /** Regex source for the provenance line the search hit points at — text
       *  that exists ONLY in the target document, so seeing it in the reader
       *  proves the jump landed on the right doc at the right content. */
      readonly provenancePattern: string;
    }
  | {
      /** **P7 W5.** Switch the interpretation through the real *Semantic view* sheet
       *  and assert the switch took effect where the user sees it: the lens
       *  band naming {@link schema} and reporting how many objects that
       *  interpretation holds. Asserting the sheet listed the schema would only
       *  test the picker. */
      readonly kind: 'viewAs';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** The registered schema to read the corpus as. */
      readonly schema: string;
      /** Regex source the lens band's text must match once the lens is on —
       *  includes the object count, so an empty interpretation cannot pass. */
      readonly bandPattern: string;
    }
  | {
      /** **P7 W5.** Select a passage in the reader and assert the Semantic
       *  Inspector reports what was derived FROM THOSE LINES — the claim's own
       *  sentence and its citation back to the file and range. */
      readonly kind: 'inspectPassage';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** The passage to select, named by the words it renders — a double-click
       *  lands on it and selects a word there. Addressed by its text rather
       *  than by a block ordinal because the reading column's block wrappers
       *  are generic `<div>`s that WebKit omits from the AX tree, so the macOS
       *  leg has no class to reach them by; naming the passage is also what the
       *  assertion below is about ("what was derived from THESE lines"). */
      readonly selectText: string;
      /** Regex source the rail's text must match (a claim's rendered sentence). */
      readonly claimPattern: string;
      /** Regex source for the claim's citation back to the source. */
      readonly citationPattern: string;
    }
  | {
      /** **P8.** Query the active Markdown document through the shipping
       * Tauri command, then inspect the one answer as result, source execution,
       * field provenance, and persisted semantic schema. */
      readonly kind: 'inspectFederation';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** Regex source matched against the Result tab. */
      readonly resultPattern: string;
      /** Regex source proving all physical source reports are present. */
      readonly sourcesPattern: string;
      /** Regex source proving the winner and an observed loser stay visible. */
      readonly provenancePattern: string;
      /** Regex source proving the schema identity/version is rendered. */
      readonly schemaPattern: string;
    }
  | {
      /** **Task 10.** Open a local document from the unified Library answer,
       * then inspect that exact answer and prove the local edit capability is
       * exposed without a second document read. */
      readonly kind: 'inspectLibraryDocument';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** Visible semantic title used to select the Library result. */
      readonly document: string;
      /** Regex source matched against the reader and local edit affordance. */
      readonly readerPattern: string;
      /** Regex source matched against the exact answer's provenance tab. */
      readonly provenancePattern: string;
      /** Regex source matched against the pinned schema tab. */
      readonly schemaPattern: string;
    }
  | {
      /** **P7 W5.** Open the Constraint Explorer, click a violation's
       *  click-to-source button, and assert the reader landed on the WHOLE
       *  cited range.
       *
       *  This is the regression that shipped and was fixed: the located span
       *  reached `onJump` intact and was then truncated to its first line. So
       *  the assertion is deliberately at the END of the chain — the blocks the
       *  reader actually marks — and demands text from BOTH ends of the range.
       *  A truncated jump marks the opening block only, and {@link tailPattern}
       *  is missing. */
      readonly kind: 'constraintJump';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      /** Regex source the constraint sheet must match (the broken law). */
      readonly lawPattern: string;
      /** Substring identifying WHICH jump button to click, by what it cites. */
      readonly jumpCiting: string;
      /** Regex source for text at the START of the cited range. */
      readonly headPattern: string;
      /** Regex source for text at the END of the cited range — the assertion a
       *  truncated span fails. */
      readonly tailPattern: string;
      /** Minimum number of blocks the range must mark (> 1: a single-block
       *  answer is what truncation looks like). */
      readonly minLocatedBlocks: number;
    }
  | {
      /** **P7 W5.** Run a query through the Query Explanation sheet and assert
       *  the ranking is actually explained: which arms matched, and the
       *  chunk → section → document → file provenance chain. */
      readonly kind: 'queryExplanation';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      readonly query: string;
      /** Regex source for the arms sentence. */
      readonly armsPattern: string;
      /** Regex source for the provenance chain. */
      readonly chainPattern: string;
    }
  | {
      /** Prove the native titlebar is genuinely, and *locally*, draggable. Two
       *  synthetic CoreGraphics gestures with the SAME delta:
       *    (1) started ON the titlebar → the window moves by ~the delta; and
       *    (2) NEGATIVE CONTROL — started in the CONTENT area → the window does
       *        NOT move.
       *  Together they show the movement came from the drag GESTURE localized to
       *  the titlebar, not from a reposition or a whole-window drag region. THE
       *  assertion a "process booted" check can't make. */
      readonly kind: 'dragWindow';
      readonly id: string;
      readonly title: string;
      readonly channel: 'window';
      readonly dx: number;
      readonly dy: number;
    }
  | {
      /** Resize the native window; assert its size changed. */
      readonly kind: 'resizeWindow';
      readonly id: string;
      readonly title: string;
      readonly channel: 'window';
      readonly dw: number;
      readonly dh: number;
    }
  | {
      /** Click the native red close button; assert the process is gone. The
       *  regression that shipped a window you could not close. */
      readonly kind: 'closeWindow';
      readonly id: string;
      readonly title: string;
      readonly channel: 'window';
    };

/** Fixture + timing configuration the flow is parameterised over. */
export interface FlowConfig {
  /** Repo-relative path to the committed workspace corpus. */
  readonly corpusRelPath: string;
  /** The Tauri bundle identifier — locates the per-OS plugin-store dir. */
  readonly bundleIdentifier: string;
  /** How long to wait for the (hash-embed) index to settle, ms. */
  readonly indexTimeoutMs: number;
}

/** The only Tauri identity native E2E is allowed to seed or launch. */
export const E2E_BUNDLE_IDENTIFIER = 'dev.mdreader.app.e2e';
/** Visible product name for the isolated native E2E bundle. */
export const E2E_PRODUCT_NAME = 'MDReader E2E';
/** Tauri's exact non-production executable name for the isolated build. */
export const E2E_PROCESS_NAME = 'mdreader-e2e';

export const FLOW_CONFIG: FlowConfig = {
  corpusRelPath: 'fixtures/corpus',
  bundleIdentifier: E2E_BUNDLE_IDENTIFIER,
  indexTimeoutMs: 30_000,
};

/**
 * THE core flow, in order. This is the checklist the milestone exists to
 * automate: grant a workspace → the index reports "Indexed N files" → a document
 * opens and renders → Cmd+K search returns a hit → Enter lands on its provenance
 * line → the window drags (position changes) → resizes → and the red-X actually
 * closes the app.
 *
 * The two `'window'` steps at the end — `dragWindow` and `closeWindow` — are the
 * ones that would have caught the dead-titlebar regression, so they carry the
 * load-bearing assertions in the macOS driver.
 */
export const FLOW: readonly Step[] = [
  {
    kind: 'launch',
    id: 'launch',
    title: 'Launch the app against the seeded fixture workspace',
    channel: 'window',
  },
  {
    kind: 'grantWorkspace',
    id: 'grant-workspace',
    title: 'Fixture workspace is granted — the file tree is populated',
    channel: 'dom',
    // The fixture corpus has 45 markdown files across several directories, all
    // expanded by default, so a populated tree renders many rows. 5 is a floor
    // that is safely below the rendered set on every platform yet high enough to
    // be a real "the tree is populated" assertion, not "one stray row exists".
    minFiles: 5,
  },
  {
    kind: 'waitIndexed',
    id: 'wait-indexed',
    title: 'The index settles: status shows "Indexed N files"',
    channel: 'dom',
    pattern: 'Indexed\\s+\\d+\\s+files',
  },
  {
    kind: 'openDoc',
    id: 'open-doc',
    title: 'Open a document (Cmd+P) — it renders',
    channel: 'dom',
    // Fuzzy-matches sample-project/overview.md, whose H1 is "Sample Project".
    query: 'overview',
    headingPattern: 'Sample Project',
  },
  {
    kind: 'search',
    id: 'search',
    title: 'Cmd+K search returns a hit',
    channel: 'dom',
    // "pluggable" occurs only in notes/zettel/node-storage.md, so it is the
    // deterministic top hit — the large log fixture never outranks it.
    query: 'pluggable',
  },
  {
    kind: 'jumpToHit',
    id: 'jump-to-hit',
    title: 'Enter jumps to the hit’s provenance line',
    channel: 'dom',
    // "pluggable" occurs only in notes/zettel/node-storage.md (H1 "Storage"), so
    // the top hit lands there and the reader must show that heading plus the
    // exact provenance line — text that appears nowhere else in the corpus.
    headingPattern: 'Storage',
    provenancePattern: 'Storage is pluggable',
  },
  // ---- P7 W5: the decisive scenario, driven through the shipping UI --------
  // The Rust half (`mdreader-ingest/tests/decisive_scenario.rs`) proves the
  // claim against the engine. These steps prove the same corpus is READABLE
  // that way in the app a user actually runs: the interpretation switches, a
  // passage reports what was derived from it, a broken law lands on the exact
  // lines it came from, and a ranking explains itself. The corpus is seeded
  // with its `architecture_overlay` claims already REVIEWED (see
  // `runners/macos.ts`), because a law can only break over facts a human let
  // into the graph.
  {
    kind: 'openDoc',
    id: 'open-arch-doc',
    title: 'Open the architecture document the broken law comes from',
    channel: 'dom',
    // `fixtures/corpus/docs/arch/broken-app.md`, H1 "broken-app".
    query: 'broken-app',
    headingPattern: 'broken-app',
  },
  {
    kind: 'viewAs',
    id: 'view-as',
    title: 'Semantic view switches the interpretation — the lens band says so',
    channel: 'dom',
    schema: 'architecture_overlay',
    // 12 committed objects for this corpus; the count is read live, so the
    // pattern only insists it is a real, non-zero number. `\s+` and not a
    // literal space because the count and its unit are separate text nodes:
    // the DOM renders "12 in the graph", and macOS's AX tree hands the two
    // nodes back on separate lines. Either way a zero interpretation still
    // cannot satisfy it.
    bandPattern: 'architecture_overlay[\\s\\S]*[1-9]\\d*\\s+in the graph',
  },
  {
    kind: 'inspectFederation',
    id: 'federated-document',
    title: 'The active Document is assembled across filesystem, Git, and frontmatter',
    channel: 'dom',
    resultPattern:
      'Federated document[\\s\\S]*Complete[\\s\\S]*title[\\s\\S]*broken-app[\\s\\S]*from frontmatter',
    sourcesPattern:
      'filesystem[\\s\\S]*ran[\\s\\S]*git[\\s\\S]*ran[\\s\\S]*frontmatter[\\s\\S]*ran',
    provenancePattern:
      'modifiedAt[\\s\\S]*filesystem\\.modifiedAt[\\s\\S]*selected[\\s\\S]*git\\.modifiedAt[\\s\\S]*also observed',
    schemaPattern:
      'document[\\s\\S]*(v\\d+|not catalogued yet)[\\s\\S]*semantic interpretation[\\s\\S]*not the Markdown file',
  },
  {
    kind: 'inspectPassage',
    id: 'inspect-passage',
    title: 'The Semantic Inspector reports what was derived from the passage',
    channel: 'dom',
    // The opening sentence of `docs/arch/broken-app.md` — the document on
    // screen from the previous step, and the very passage the overlay's
    // `Component` claim was extracted from.
    selectText: 'Claims to depend on catdb-core',
    // broken-app.md yields a whole-file `Component` claim, approved by the
    // seeded reviewer, so the rail must show the claim AND that it is in force.
    claimPattern: 'Component',
    citationPattern: 'broken-app\\.md:\\d+',
  },
  {
    kind: 'constraintJump',
    id: 'constraint-jump',
    title: 'A broken law jumps to the WHOLE range it came from',
    channel: 'dom',
    lawPattern: 'Dependency',
    // The violation's right-hand endpoint is the `catdb-core` component, cited
    // to the whole of its document — a genuinely multi-line, multi-block range,
    // and a different document from the one on screen, so the jump crosses
    // documents as well as spanning lines.
    jumpCiting: 'catdb-core.md',
    headPattern: 'The categorical kernel',
    // The last section of that file. Only reachable if the range's END survived
    // the trip from the sheet to the reader.
    tailPattern: 'Undecidable in general',
    minLocatedBlocks: 2,
  },
  {
    kind: 'queryExplanation',
    id: 'query-explanation',
    title: 'A ranking explains itself, arms and provenance chain',
    channel: 'dom',
    query: 'hybrid retrieval',
    armsPattern: '(Both arms matched|Only the (lexical|semantic) arm matched)',
    chainPattern: 'chunk[\\s\\S]*section[\\s\\S]*document',
  },
  {
    kind: 'inspectLibraryDocument',
    id: 'unified-library-document',
    title: 'The unified Library opens and inspects the same local Document answer',
    channel: 'dom',
    document: 'catdb-core',
    readerPattern: 'Edit local file[\\s\\S]*catdb-core',
    provenancePattern:
      'title[\\s\\S]*\\.title[\\s\\S]*record[^\\n]*catdb-core\\.md',
    schemaPattern:
      'Catalog authority[\\s\\S]*Schema[\\s\\S]*Mappings[\\s\\S]*Sources[\\s\\S]*Policies',
  },
  {
    kind: 'dragWindow',
    id: 'drag-window',
    title: 'Drag the window — its position changes',
    channel: 'window',
    dx: 140,
    dy: 90,
  },
  {
    kind: 'resizeWindow',
    id: 'resize-window',
    title: 'Resize the window — its size changes',
    channel: 'window',
    dw: 120,
    dh: 80,
  },
  {
    kind: 'closeWindow',
    id: 'close-window',
    title: 'Red-X closes the app — the process is gone',
    channel: 'window',
  },
] as const;

/** The step kinds that drive the P7 semantic surfaces. A driver that cannot
 *  assert them registers them PENDING with a reason (see `drivers/types.ts`),
 *  the same explicit-boundary rule the native-window steps follow — never a
 *  silent skip. */
export const SEMANTIC_KINDS: readonly Step['kind'][] = [
  'viewAs',
  'inspectFederation',
  'inspectLibraryDocument',
  'inspectPassage',
  'constraintJump',
  'queryExplanation',
];

/** Steps that operate on the webview DOM (the WebdriverIO suite's domain). */
export const DOM_STEPS: readonly Step[] = FLOW.filter((s) => s.channel === 'dom');

/** Steps that operate on the native window (the macOS driver's rigorous domain). */
export const WINDOW_STEPS: readonly Step[] = FLOW.filter((s) => s.channel === 'window');
