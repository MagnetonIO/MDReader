/** Current desktop acceptance: native folder membership, SQLite text search,
 * exact native reading receipts and genuine window behavior. Mobile has its
 * own device-import flow; removed CatDB interpretation surfaces are excluded. */
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
 *  here is grounded in the real shipping DOM (see `ui/src/shell`
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

} as const satisfies Record<string, DomSelector>;

export type SelectorName = keyof typeof SELECTORS;
export type Chord = 'Mod+P' | 'Mod+K' | 'Enter' | 'Escape';
export type Channel = 'dom' | 'window';
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
      readonly intent: 'Exact';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      readonly query: string;
      /** The one corpus document `query` occurs in. */
      readonly expectedPath: string;
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
      readonly kind: 'inspectReceipt';
      readonly id: string;
      readonly title: string;
      readonly channel: 'dom';
      readonly path: string;
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
  /** How long to wait for the text index to settle, ms. */
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
    title: 'Text indexing settles; disabled semantic model remains explicit',
    channel: 'dom',
    pattern: '(?:Indexed(?:\\s+\\d+\\s+files)?|Embedding model unavailable)',
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
    expectedPath: 'notes/zettel/node-storage.md',
    intent: 'Exact',
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
  {
    kind: 'inspectReceipt', id: 'exact-reading-receipt',
    title: 'Document details match the native exact original receipt',
    channel: 'dom', path: 'notes/zettel/node-storage.md',
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
export const DOM_STEPS: readonly Step[] = FLOW.filter((step) => step.channel === 'dom');
export const WINDOW_STEPS: readonly Step[] = FLOW.filter((step) => step.channel === 'window');
