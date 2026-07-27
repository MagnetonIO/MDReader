/**
 * REAL-ENGINE mode for the Playwright harness.
 *
 * The default harness (`./harness.ts`) answers every Tauri command from an
 * in-page fixture. That is the right tool for layout, focus mode, selection and
 * the honest-rendering rules — but it can never catch a WRONG answer, because
 * the answer is whatever the fixture says it is. A fixture can claim any hit,
 * any score, any provenance chain you like.
 *
 * This module answers a subset of the same commands from a **genuinely running
 * `catdb-server`** — the axum `/v1` surface booted by `catdb serve` over a real
 * SQLite workspace, indexed from a real corpus. Every id, line number, score,
 * rank and provenance chain the UI renders in this mode came out of real
 * storage. Loaded only when the page is opened with `?realengine`; see
 * `../../e2e/real-engine.setup.ts` for how the server is started and stopped.
 *
 * ## Why HTTP, and what that does and does not prove
 *
 * `catdb-server` and the desktop shell's Tauri commands are two projections of
 * the SAME engine call. For the surface used here they are the same data:
 * `catdb-server`'s `HitExplainDto`/`ProvenanceChainDto`
 * (`catdb/crates/catdb-server/src/dto.rs`) and the shell's
 * `ExplainDto`/`ProvenanceDto` (`apps/mdreader/src-tauri/src/search.rs`) are
 * field-for-field the same projection of `catdb_engine::search::HitExplain`.
 *
 * Three honest differences, all handled here rather than hidden:
 *  1. The shell's hit carries `abs_path`, `name` and `mtime`, which are derived
 *     from the FILESYSTEM, not the engine. `/v1` has no such fields, so they are
 *     reconstructed from the source root the engine itself reports (`abs_path`,
 *     `name`) or reported as the honest absence (`mtime: null`).
 *  2. `/v1` omits absent optionals (`skip_serializing_if`) where the shell emits
 *     explicit `null`. Normalised below with `?? null` — never with `?? 0`.
 *  3. `/v1` has no counterpart at all for the P7 overlay commands, for wikilink
 *     resolution, or for writing a file. Those FAIL, loudly (see
 *     {@link unsupported}) — they are never quietly answered from a fixture.
 *     Shell-only library admission below is a narrow transport scaffold: its
 *     root and rows are still learned from the real engine, and no engine answer
 *     is fabricated. Native persistence/grant serialization remains the native
 *     harness's acceptance bar.
 *
 * So: this proves the UI renders REAL engine answers. It does not prove the
 * Rust Tauri command layer's own serialization — that is the native leg's job
 * (`apps/mdreader/e2e-native`). See docs/testing/gui-e2e.md.
 *
 * ## Fail closed
 *
 * Nothing here falls back to a fixture, ever. If the server is down, answers
 * something other than 2xx JSON, or holds an empty index, every affected command
 * REJECTS with a message beginning {@link FAILURE_MARK}. A real-engine spec that
 * quietly degraded to mocks would report success for a path it never exercised.
 */

/**
 * The same-origin prefix the Vite dev server proxies to the running
 * catdb-server (see `vite.config.ts`).
 *
 * It has to be same-origin: `catdb-server` installs no CORS layer at all
 * (`catdb/crates/catdb-server/src/rest.rs` mounts only the auth seam), and every
 * `/v1` route is a `POST` with `content-type: application/json` — not a simple
 * request, so a cross-origin call would need a preflight, and `OPTIONS /v1/…`
 * answers `405`. The proxy is a transparent reverse proxy: the page issues a
 * genuine `fetch`, and a genuine HTTP round trip reaches the real server.
 */
export const CATDB_PROXY_PREFIX = '/__catdb';

/** Every real-engine failure message starts with this, so it is unmistakable. */
export const FAILURE_MARK = 'REAL-ENGINE MODE';

/**
 * The window-side record of what this mode refused to invent.
 *
 * A rejected IPC promise reaches the UI as whatever that surface does with an
 * error — which, for a tree that never populates, looks like a slow render. A
 * spec reading only the DOM would time out and report "element not found", which
 * says nothing about WHY. So both refusals are recorded here, and the specs read
 * them: a real-engine failure is then named in the test output rather than
 * inferred from an absence.
 */
export interface RealEngineLog {
  /** The server could not be reached, or did not answer usefully. */
  failures: string[];
  /** Commands `/v1` does not serve, refused by design (see `unsupported`). */
  unsupported: string[];
}

function log(): RealEngineLog {
  const store = window as unknown as { __REAL_ENGINE__?: RealEngineLog };
  store.__REAL_ENGINE__ ??= { failures: [], unsupported: [] };
  return store.__REAL_ENGINE__;
}

/** Raise a real-engine failure. Never returns; never answers from a fixture. */
function fail(detail: string): never {
  const message = `${FAILURE_MARK}: ${detail}`;
  log().failures.push(message);
  throw new Error(message);
}

// ---------------------------------------------------------------------------
// the wire
// ---------------------------------------------------------------------------

/** `catdb-server`'s `HitBoostsDto` — plain `f32`s, so never null on the wire. */
interface WireBoosts {
  title: number;
  path: number;
  recency: number;
  graph: number;
}

/** `catdb-server`'s `ProvenanceChainDto`. Optionals are omitted, not nulled. */
interface WireProvenance {
  chunk_id: string;
  section_id: string;
  document_id: string;
  file_path: string;
  source_id?: string;
  source_root?: string;
}

/** `catdb-server`'s `HitExplainDto`. */
interface WireExplain {
  lex_rank?: number;
  sem_rank?: number;
  rrf_score: number;
  boosts: WireBoosts;
  final_score: number;
  index_version?: string;
  embedding_model?: string;
  provenance: WireProvenance;
}

/** `catdb-server`'s `SearchHitDto`. */
interface WireHit {
  chunk_id: string;
  document_id: string;
  file_path: string;
  heading_path: string[];
  start_line: number;
  end_line: number;
  score: number;
  rrf_score: number;
  boosts: WireBoosts;
  lex_rank?: number;
  sem_rank?: number;
  snippet: string;
  explain?: WireExplain;
}

interface WireSearchResponse {
  hits: WireHit[];
  count: number;
}

interface WireReadResponse {
  document: { id: string; source_id?: string; file_path: string; content_hash: string };
  sections: { id: string; heading_path: string[]; start_line: number; end_line: number }[];
  content: string;
}

interface WireRelated {
  document_id: string;
  file_path: string;
  score: number;
  components: { graph: number; concept: number; vector: number };
}

interface WireRelatedResponse {
  document: WireReadResponse['document'];
  related: WireRelated[];
}

interface WireIndexStatus {
  phase: string;
  files_scanned: number;
  files_done: number;
  chunks_embedded: number;
  chunks_total?: number;
  error?: string;
}

/**
 * One `/v1` call. Anything other than a 2xx carrying parseable JSON is a
 * FAILURE, surfaced verbatim — never an empty result that would read like
 * "no matches".
 */
async function call<T>(route: string, body?: unknown): Promise<T> {
  const url = `${CATDB_PROXY_PREFIX}${route}`;
  let response: Response;
  try {
    response = await fetch(
      url,
      body === undefined
        ? { method: 'GET' }
        : {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(body),
          },
    );
  } catch (cause) {
    return fail(
      `no catdb-server answered ${url} (${cause instanceof Error ? cause.message : String(cause)}). ` +
        'The server is started by the Playwright global setup — see e2e/real-engine.setup.ts. ' +
        'Nothing was answered from a fixture.',
    );
  }
  const text = await response.text();
  if (!response.ok) {
    return fail(
      `${url} came back ${response.status}` +
        (text.length === 0 ? ' with an empty body' : `: ${text.slice(0, 400)}`) +
        '. This path goes through the Vite dev proxy, so a 5xx with an empty body means no ' +
        'catdb-server was listening on the proxied port at all — the server is started by the ' +
        'Playwright global setup (e2e/support/catdb-server.ts). Nothing was answered from a fixture.',
    );
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    return fail(`catdb-server answered ${url} with a body that is not JSON: ${text.slice(0, 400)}`);
  }
}

// ---------------------------------------------------------------------------
// the workspace, learned from the engine itself
// ---------------------------------------------------------------------------

/**
 * The corpus root and the documents the index actually holds.
 *
 * Both are read back OUT OF THE ENGINE rather than configured into the page:
 * `type:document` is a document-granularity query, so its hits enumerate the
 * indexed corpus, and each hit's explain chain carries the `source_root` the
 * ingest recorded. That is deliberate — the page cannot know the run's tempdir,
 * so a workspace root appearing in the UI is evidence the answer came from a
 * real index.
 *
 * `/v1` has no document-enumeration endpoint (the platform SDK has no
 * `listDocuments` either, and `HttpBackend.listSources` is operator-supplied for
 * exactly this reason), so this is the honest substitute: the tree lists the
 * documents the engine can retrieve, not a filesystem walk.
 */
interface Workspace {
  root: string;
  /** Source-relative stored paths, sorted. */
  documents: string[];
}

let workspacePromise: Promise<Workspace> | null = null;
let realLibraryAdded = false;
let realLibraryActive = false;
let realDiagnosticLevel: 'errors' | 'standard' | 'verbose' = 'standard';

async function workspace(): Promise<Workspace> {
  workspacePromise ??= (async () => {
    const response = await call<WireSearchResponse>('/v1/search', {
      q: 'type:document',
      limit: 1000,
      explain: true,
    });
    if (response.hits.length === 0) {
      return fail(
        'catdb-server is running but its index holds NO documents — a `type:document` query ' +
          'returned nothing. The workspace was never indexed, or was indexed from an empty ' +
          'directory. Refusing to present an empty index as a workspace.',
      );
    }
    const root = response.hits.find((hit) => hit.explain?.provenance.source_root)?.explain
      ?.provenance.source_root;
    if (root === undefined) {
      return fail(
        'no hit carried a `source_root`, so the corpus root the engine ingested is unknown. ' +
          'Refusing to guess it.',
      );
    }
    const documents = [...new Set(response.hits.map((hit) => hit.file_path))].sort();
    return { root, documents };
  })();
  return workspacePromise;
}

/** `root/relative`, the absolute path the reader opens. */
function absolute(root: string, relative: string): string {
  return `${root}/${relative}`;
}

/** The stored (source-relative) path for an absolute one under `root`. */
function relativeTo(root: string, path: string): string {
  const prefix = `${root}/`;
  return path.startsWith(prefix) ? path.slice(prefix.length) : path;
}

function basename(path: string): string {
  return path.slice(path.lastIndexOf('/') + 1);
}

function dirname(path: string): string {
  const cut = path.lastIndexOf('/');
  return cut <= 0 ? path : path.slice(0, cut);
}

async function realLibrarySource() {
  const { root } = await workspace();
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(root));
  const id = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join(
    '',
  );
  return {
    id,
    name: basename(root),
    path: root,
    state: { kind: 'available' },
    index_state: 'present',
    active: realLibraryActive,
    restore_candidate: realLibraryActive,
  };
}

async function realLibrarySnapshot() {
  return {
    version: 3,
    sources: realLibraryAdded ? [await realLibrarySource()] : [],
    cleanup: { kind: 'complete' },
  };
}

// ---------------------------------------------------------------------------
// wire → the shell's command shapes
// ---------------------------------------------------------------------------

/**
 * A `/v1` hit in the shell's `SearchHitDto` shape.
 *
 * `abs_path` and `name` are reconstructed from the source root the ENGINE
 * reported; `mtime` is `null` — the honest absence, because `/v1` carries no
 * file mtime and inventing one would be exactly the fabrication this mode
 * exists to remove. The bindings type it `number | null`, and the shell's
 * `normalizeFile`/`normalizeHit` already read `?? 0`.
 */
function toHit(root: string, hit: WireHit) {
  const explain = hit.explain;
  return {
    chunk_id: hit.chunk_id,
    document_id: hit.document_id,
    file_path: hit.file_path,
    abs_path: absolute(root, hit.file_path),
    name: basename(hit.file_path),
    heading_path: hit.heading_path,
    start_line: hit.start_line,
    end_line: hit.end_line,
    score: hit.score,
    rrf_score: hit.rrf_score,
    boosts: hit.boosts,
    lex_rank: hit.lex_rank ?? null,
    sem_rank: hit.sem_rank ?? null,
    snippet: hit.snippet,
    mtime: null,
    explain:
      explain === undefined
        ? null
        : {
            lex_rank: explain.lex_rank ?? null,
            sem_rank: explain.sem_rank ?? null,
            rrf_score: explain.rrf_score,
            boosts: explain.boosts,
            final_score: explain.final_score,
            index_version: explain.index_version ?? null,
            embedding_model: explain.embedding_model ?? null,
            provenance: {
              chunk_id: explain.provenance.chunk_id,
              section_id: explain.provenance.section_id,
              document_id: explain.provenance.document_id,
              file_path: explain.provenance.file_path,
              source_id: explain.provenance.source_id ?? null,
              source_root: explain.provenance.source_root ?? null,
            },
          },
  };
}

/**
 * `/v1/index_status`'s phase in the shell's `ImportProgress` vocabulary.
 *
 * The two enumerations do not coincide, and the mapping states that rather than
 * smoothing it: the server's in-flight phases (`scanning`/`syncing`/
 * `embedding`/`queued`) all mean "importing" to the shell, `failed` is `error`,
 * and `cancelled` — which the shell has no word for — is reported as `error`
 * with the reason, never as `done`.
 */
function toImportProgress(status: WireIndexStatus) {
  const phase =
    status.phase === 'done'
      ? 'done'
      : status.phase === 'idle'
        ? 'idle'
        : status.phase === 'failed' || status.phase === 'cancelled'
          ? 'error'
          : 'importing';
  return {
    phase,
    files_done: status.files_done,
    files_total: status.files_scanned,
    chunks_embedded: status.chunks_embedded,
    error:
      phase === 'error'
        ? (status.error ?? `the index job ended in phase "${status.phase}"`)
        : null,
  };
}

// ---------------------------------------------------------------------------
// the responder
// ---------------------------------------------------------------------------

/**
 * Commands the `/v1` surface simply does not serve. Answering these from a
 * fixture while claiming to be in real-engine mode is the precise failure this
 * mode exists to prevent, so they REJECT instead — naming the command and why.
 */
function unsupported(cmd: string): never {
  const message =
    `${FAILURE_MARK}: the command "${cmd}" has no /v1 counterpart — catdb-server serves ` +
    'search/read/related/index_status, not the overlay (claims, schemas, constraints), wikilink ' +
    'resolution or file writes. This surface can only be driven in MOCKED mode; it is NOT being ' +
    'faked here.';
  // Logged apart from {@link fail}: this is a documented boundary of the mode,
  // not the server misbehaving, and a spec must be able to tell the two apart.
  log().unsupported.push(message);
  throw new Error(message);
}

/**
 * Answer one Tauri command from the live server. Returns whatever the mocked
 * handler would have returned for that command, and rejects otherwise.
 */
export async function answerFromRealEngine(cmd: string, args: unknown): Promise<unknown> {
  switch (cmd) {
    case 'list_library_sources':
      return realLibrarySnapshot();

    case 'pick_and_add_library_source': {
      const added = !realLibraryAdded;
      realLibraryAdded = true;
      return { source: await realLibrarySource(), added };
    }

    case 'reconcile_library_restore': {
      const source = await realLibrarySource();
      return realLibraryAdded && (args as { sessionRoot: string | null }).sessionRoot === source.path
        ? source
        : null;
    }

    case 'watch_library_source': {
      const source = await realLibrarySource();
      if ((args as { sourceId: string }).sourceId !== source.id) {
        return fail('the shell attempted to watch a library ID that was not derived from the real root');
      }
      return { root: source.path, generation: 1 };
    }

    case 'grant_library_source': {
      const source = await realLibrarySource();
      if ((args as { sourceId: string }).sourceId !== source.id) {
        return fail('the shell attempted to grant a library ID that was not derived from the real root');
      }
      return source.path;
    }

    case 'mark_library_source_active': {
      const source = await realLibrarySource();
      if ((args as { sourceId: string }).sourceId !== source.id) {
        return fail('the shell attempted to activate a library ID that was not derived from the real root');
      }
      realLibraryActive = true;
      return realLibrarySnapshot();
    }

    case 'forget_library_source':
      if (realLibraryActive) {
        throw { kind: 'active-source', source_id: (await realLibrarySource()).id };
      }
      realLibraryAdded = false;
      return realLibrarySnapshot();

    case 'record_diagnostic_event':
    case 'reveal_diagnostics':
      return null;

    case 'diagnostics_info':
      return {
        level: realDiagnosticLevel,
        max_file_bytes: 256 * 1024,
        max_retained_files: 3,
        max_export_bytes: 512 * 1024,
      };

    case 'set_diagnostics_level':
      realDiagnosticLevel = (args as {
        level: 'errors' | 'standard' | 'verbose';
      }).level;
      return {
        level: realDiagnosticLevel,
        max_file_bytes: 256 * 1024,
        max_retained_files: 3,
        max_export_bytes: 512 * 1024,
      };

    case 'export_diagnostics':
      return { status: 'cancelled', bytes_written: 0 };

    case 'clear_diagnostics':
      if (!(args as { confirmed: boolean }).confirmed) {
        throw { kind: 'confirmation-required' };
      }
      return {
        level: realDiagnosticLevel,
        max_file_bytes: 256 * 1024,
        max_retained_files: 3,
        max_export_bytes: 512 * 1024,
      };

    case 'pick_workspace_folder':
      return (await workspace()).root;

    case 'set_workspace':
      return (args as { root: string }).root;

    case 'watch_workspace':
      // No filesystem watcher exists over HTTP; the grant shape is what the
      // shell adopts as the active workspace, and generation 1 is the first.
      return { root: (args as { root: string }).root, generation: 1 };

    case 'list_workspace_files': {
      const { root, documents } = await workspace();
      return {
        entries: documents.map((relative) => {
          const path = absolute(root, relative);
          return {
            path,
            name: basename(path),
            dir: dirname(path),
            // No mtime and no size on the wire: absent, not zero.
            mtime: null,
            size: null,
            // The ingest scan drops ignored files before they reach the index,
            // so anything the index holds is by construction not ignored.
            ignored: false,
          };
        }),
        // True in the strongest sense available: these rows ARE the index.
        from_index: true,
      };
    }

    case 'start_import':
    case 'import_status': {
      // The corpus was imported by the Playwright global setup before the
      // browser ever launched, so this reports the REAL job state the server
      // holds — it does not re-run an import behind the UI's back.
      const status = await call<WireIndexStatus>('/v1/index_status');
      return toImportProgress(status);
    }

    case 'read_doc': {
      const { root } = await workspace();
      const path = (args as { path: string }).path;
      const response = await call<WireReadResponse>('/v1/read', {
        reference: relativeTo(root, path),
      });
      return {
        // The body the engine reconstructs from its stored chunks. NOTE: this
        // is not necessarily the file's bytes — frontmatter is not stored, so a
        // document with frontmatter starts at its first heading and its line
        // numbers are offset from the raw file's. Real, but not the same read
        // the desktop shell's `read_doc` performs (that one opens the file).
        content: response.content,
        mtime: null,
        // A real blake3 digest out of storage, not an opaque mock token.
        hash: response.document.content_hash,
      };
    }

    case 'search': {
      const { root } = await workspace();
      const { query, options } = args as {
        query: string;
        options: { limit: number; lexical_only: boolean; explain: boolean };
      };
      const response = await call<WireSearchResponse>('/v1/search', {
        q: query,
        limit: options.limit,
        lexical_only: options.lexical_only,
        explain: options.explain,
      });
      return {
        hits: response.hits.map((hit) => toHit(root, hit)),
        // The server ran the hybrid pass it was asked for; there is no
        // "the model would not load" state to report over HTTP.
        semantic_unavailable: null,
      };
    }

    case 'explain_query': {
      const { root } = await workspace();
      const { query, limit } = args as { query: string; limit: number };
      // `/v1/explain` compiles a PLAN and executes nothing; the per-hit
      // breakdown the shell's `explain_query` returns is `/v1/search` with
      // `explain: true` — the same engine call, the same `HitExplain`.
      const response = await call<WireSearchResponse>('/v1/search', {
        q: query,
        limit,
        explain: true,
      });
      const missing = response.hits.filter((hit) => hit.explain === undefined).length;
      if (missing > 0) {
        return fail(
          `${missing} of ${response.hits.length} hits came back with no explain breakdown, ` +
            'though one was requested. Refusing to render an unexplained ranking as an explained one.',
        );
      }
      return response.hits.map((hit) => toHit(root, hit));
    }

    case 'related': {
      const { root } = await workspace();
      const { docPath, limit } = args as { docPath: string; limit: number };
      const response = await call<WireRelatedResponse>('/v1/related', {
        reference: relativeTo(root, docPath),
        limit,
      });
      return response.related.map((doc) => ({
        document_id: doc.document_id,
        file_path: doc.file_path,
        abs_path: absolute(root, doc.file_path),
        name: basename(doc.file_path),
        score: doc.score,
        components: doc.components,
      }));
    }

    case 'ping':
      // Prove the round trip reaches the server rather than echoing in-page.
      await call<WireIndexStatus>('/v1/index_status');
      return { echoed: (args as { msg: string }).msg, version: 'catdb-server' };

    // --- surfaces /v1 does not serve ------------------------------------
    case 'resolve_wikilink':
    case 'save_doc':
    case 'list_schemas':
    case 'describe_schema':
    case 'project_corpus':
    case 'inspect_span':
    case 'approve_fact':
    case 'reject_fact':
    case 'check_constraints':
      return unsupported(cmd);

    default:
      // Plugin channels (store, event) are inert here exactly as in the mocked
      // harness — they are Tauri plumbing, not engine answers.
      if (cmd.startsWith('plugin:store|load')) return 1;
      if (cmd.startsWith('plugin:store|')) return null;
      if (cmd.startsWith('plugin:event|')) return 0;
      return unsupported(cmd);
  }
}
