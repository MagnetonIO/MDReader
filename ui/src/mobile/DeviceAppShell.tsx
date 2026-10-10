import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { MarkdownReader, PlatformProvider } from "@mdreader/ui";
import type {
  MarkdownReaderHandle,
  Platform,
  ScrollAnchor,
} from "@mdreader/ui";
import { parseMarkdownAuto } from "@mdreader/markdown/worker";
import { resolveHeading } from "@mdreader/markdown";
import type { ParsedDoc } from "@mdreader/markdown";
import { commands } from "../bindings";
import type { AssetManifest, DeviceDocument, PendingDeviceImport } from "../bindings";
import { applyTheme, useSettingsStore } from "../stores/settings";
import { loadPersisted, saveSettings } from "../shell/persistence";
import "./mobile.css";
import { DeviceSearch } from "./DeviceSearch";
import type { DeviceSearchSession } from "./DeviceSearch";
import type { DeviceReference } from "../features/device-reader/backend";
import "../features/settings/settings.css";
import { resolveImportedLink } from "./imported-links";
import { nativeAssetBackend } from "./device-assets";
import { DeviceAssetImage, DeviceAssetProvider, useDeviceAssetImages } from "./DeviceAssetImage";
import { DocumentActions } from "../features/document-transfer/DocumentActions";
import { ReadingLinkOpen } from "../features/document-transfer/ReadingLinkOpen";
import type { CanonicalReaderReference } from "../features/reader/contracts";
import { DeviceAssetsSection } from "./DeviceAssetsSection";
import { MobileSources } from "./MobileSources";
import { MobileSyncSettings } from "./MobileSyncSettings";
import { useMobileAccountSync } from "./useMobileAccountSync";
import { useMobileLibraryRefresh } from "./useMobileLibraryRefresh";
import { transferCapabilities, transferDocument } from "./document-transfer";
import type { TransferCapabilities } from "./document-transfer";
import { useIncomingMarkdown } from "./useIncomingMarkdown";
import { useMobileReadingLinks } from "./useMobileReadingLinks";
import { MobileProviderReader } from "./MobileProviderReader";
import type { ProviderReaderReference } from "../platform/providers";
import { useReaderRestoration } from "./useReaderRestoration";
import { openReaderExternal } from "../platform/external-links";

type Tab = "Library" | "Search" | "Sources" | "Settings";
async function result<T>(
  value: Promise<
    { status: "ok"; data: T } | { status: "error"; error: string }
  >,
): Promise<T> {
  const response = await value;
  if (response.status === "error") throw new Error(response.error);
  return response.data;
}
function message(cause: unknown): string {
  return cause instanceof Error
    ? cause.message
    : "Device library operation failed";
}

/** Local reading remains account-free; optional sync shares the embedded owner. */
export function DeviceAppShell({ platform }: { platform: "ios" | "android" }) {
  const [tab, setTab] = useState<Tab>("Library");
  const [linkedProvider, setLinkedProvider] = useState<ProviderReaderReference | null>(null);
  const searchSession = useRef<DeviceSearchSession>({ query: "", mode: "text" });
  const [syncSelection, setSyncSelection] = useState<DeviceReference>();
  const [syncFocus, setSyncFocus] = useState(0);
  const [documents, setDocuments] = useState<DeviceDocument[]>([]);
  const [pending, setPending] = useState<PendingDeviceImport[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [active, setActive] = useState<{
    document: DeviceDocument;
    line: number;
    heading?: string | null;
  } | null>(null);
  const [parsedResult, setParsedResult] = useState<{ id: string; revision: string; doc: ParsedDoc } | null>(null);
  const parsed = parsedResult?.id === active?.document.id && parsedResult?.revision === active?.document.revision ? parsedResult?.doc ?? null : null;
  const [readLine, setReadLine] = useState(1);
  const [readError, setReadError] = useState<string | null>(null);
  const [readAttempt, setReadAttempt] = useState(0);
  const [evidence, setEvidence] = useState(false);
  const [outline, setOutline] = useState(false);
  const [hydrated, setHydrated] = useState(false);
  const theme = useSettingsStore((state) => state.theme);
  const setTheme = useSettingsStore((state) => state.setTheme);
  const [tablet, setTablet] = useState(
    () => window.matchMedia?.("(min-width: 768px)").matches ?? false,
  );
  const [systemDark, setSystemDark] = useState(
    () => window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false,
  );
  const readEpoch = useRef(0);
  const navigationEpoch = useRef(0);
  const reader = useRef<MarkdownReaderHandle | null>(null);
  const readerHost = useRef<HTMLDivElement | null>(null);
  const position = useRef<{
    id: string;
    revision: string;
    line: number;
  } | null>(null);
  const positionTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const restoration = useReaderRestoration(reader, () => setError("The saved position could not be restored. Scroll to choose a new reading place."));

  const refresh = useMobileLibraryRefresh((value) => {
    setDocuments(value.documents);
    setPending(value.pending);
    const contains = (id: string, source: string, revision: string) => value.documents.some(
      (document) => document.id === id && document.source_id === source && document.revision === revision,
    );
    if (syncSelection && !contains(syncSelection.documentId, syncSelection.sourceId, syncSelection.revision))
      setSyncSelection(undefined);
    if (active && !contains(active.document.id, active.document.source_id, active.document.revision)) {
      // Applied replacements/deletions retire the displayed snapshot. They do
      // not grant permission to open a newer revision or save its old position.
      readEpoch.current++; navigationEpoch.current++;
      if (positionTimer.current !== null) clearTimeout(positionTimer.current);
      positionTimer.current = null; position.current = null;
      setParsedResult(null); setAssetResult(null);
      setReadError("This exact reading copy changed or was removed. Return to Library to choose an available copy.");
    } else if (active) {
      const document = value.documents.find((document) => document.id === active.document.id)!;
      setActive({ ...active, document });
    }
  });
  const observeLibrary = useCallback(async () => {
    try { await refresh(); }
    catch (cause) { setError(message(cause)); }
  }, [refresh]);
  const account = useMobileAccountSync(tab, observeLibrary);
  const transferReference = useMemo(() => active ? ({ documentId: active.document.id, sourceId: active.document.source_id, revision: active.document.revision, path: "document.md", title: active.document.name }) : null, [active?.document.id, active?.document.source_id, active?.document.revision, active?.document.name]);
  const activeId = active?.document.id;
  const activeRevision = active?.document.revision;
  const activeAvailable = Boolean(active && documents.some((document) => document.id === active.document.id
    && document.source_id === active.document.source_id && document.revision === active.document.revision));
  const activeHeading = active?.heading ?? null;
  const [assetResult, setAssetResult] = useState<AssetManifest | null>(null);
  const [assetError, setAssetError] = useState<string | null>(null);
  const [assetRefresh, setAssetRefresh] = useState(0);
  const refreshAssets = useCallback(() => {
    setAssetResult(null);
    setAssetRefresh((value) => value + 1);
  }, []);
  const assetManifest = activeAvailable && assetResult && assetResult.document_id === activeId && assetResult.document_revision === activeRevision ? assetResult : null;
  const [transferBusy, setTransferBusy] = useState(false);
  const [transferSupport, setTransferSupport] = useState<TransferCapabilities>({ export: false, share: false, readingLinks: false });
  const assetImages = useDeviceAssetImages(transferBusy || !activeAvailable ? null : assetManifest);
  useEffect(() => {
    let current = true;
    void transferCapabilities().then((value) => { if (current) setTransferSupport(value); }).catch(() => {});
    return () => { current = false; };
  }, []);
  useEffect(() => {
    let current = true;
    setAssetResult(null); setAssetError(null);
    if (!activeAvailable || !activeId || !activeRevision) return;
    void nativeAssetBackend({ operation: 'manifest', document_id: activeId, revision: activeRevision })
      .then((reply) => {
        if (!current) return;
        if (reply.operation !== 'manifest' || reply.value.document_id !== activeId || reply.value.document_revision !== activeRevision)
          throw new Error('The selected image manifest could not be verified.');
        setAssetResult(reply.value);
      }).catch((cause: unknown) => { if (current) setAssetError(message(cause)); });
    return () => { current = false; };
  }, [activeAvailable, activeId, activeRevision, assetRefresh]);
  useEffect(() => {
    let current = true;
    void refresh()
      .catch((cause) => {
        if (current) setError(message(cause));
      })
      .finally(() => {
        if (current) setLoading(false);
      });
    void loadPersisted()
      .then((value) => {
        if (current) {
          useSettingsStore.getState().hydrate(value.settings);
          setHydrated(true);
        }
      })
      .catch((cause) => {
        if (current) setError(message(cause));
      });
    return () => {
      current = false;
      readEpoch.current++;
      navigationEpoch.current++;
    };
  }, [refresh]);
  useEffect(() => applyTheme(theme), [theme]);
  useEffect(() => {
    if (!hydrated) return;
    const timer = setTimeout(
      () => void saveSettings(useSettingsStore.getState()),
      100,
    );
    return () => clearTimeout(timer);
  }, [theme, hydrated]);
  useEffect(() => {
    const wide = window.matchMedia?.("(min-width: 768px)");
    const dark = window.matchMedia?.("(prefers-color-scheme: dark)");
    const updateWide = (event: MediaQueryListEvent) => setTablet(event.matches);
    const updateDark = (event: MediaQueryListEvent) =>
      setSystemDark(event.matches);
    wide?.addEventListener("change", updateWide);
    dark?.addEventListener("change", updateDark);
    return () => {
      wide?.removeEventListener("change", updateWide);
      dark?.removeEventListener("change", updateDark);
    };
  }, []);
  const flushPosition = useCallback(async () => {
    if (positionTimer.current !== null) {
      clearTimeout(positionTimer.current);
      positionTimer.current = null;
    }
    const pending = position.current;
    position.current = null;
    if (!pending) return;
    try {
      await result(
        commands.mobileReaderPosition(
          pending.id,
          pending.revision,
          pending.line,
        ),
      );
      setDocuments((value) =>
        value.map((doc) =>
          doc.id === pending.id && doc.revision === pending.revision
            ? { ...doc, line: pending.line } : doc,
        ),
      );
    } catch (cause) {
      setError(`Reading position was not saved: ${message(cause)}`);
    }
  }, []);
  useEffect(() => {
    const flush = () => {
      if (document.visibilityState === "hidden") void flushPosition();
    };
    document.addEventListener("visibilitychange", flush);
    return () => {
      document.removeEventListener("visibilitychange", flush);
      void flushPosition();
    };
  }, [flushPosition]);
  useEffect(() => {
    const epoch = ++readEpoch.current;
    setParsedResult(null);
    setReadError(null);
    restoration.reset();
    setEvidence(false);
    setOutline(false);
    if (!activeId || !activeRevision) return;
    if (!activeAvailable) {
      setReadError("This exact reading copy changed or was removed. Return to Library to choose an available copy.");
      return;
    }
    void result(commands.mobileReaderRead(activeId, activeRevision))
      .then(async (value) => {
        const doc = await parseMarkdownAuto(value.markdown);
        if (epoch !== readEpoch.current) return;
        const heading = activeHeading ? resolveHeading(doc.outline, activeHeading) : null;
        setReadLine(activeHeading ? heading?.lineStart ?? 1 : active?.line ?? 1);
        setParsedResult({ id: activeId, revision: activeRevision, doc });
      })
      .catch((cause) => {
        if (epoch === readEpoch.current) setReadError(message(cause));
      });
    return () => {
      readEpoch.current++;
    };
  }, [activeAvailable, activeId, activeRevision, activeHeading, readAttempt]);
  const open = async (doc: DeviceDocument, line = doc.line, heading: string | null = null) => {
    const epoch = ++navigationEpoch.current;
    await flushPosition();
    if (navigationEpoch.current !== epoch) return;
    setError(null);
    setLinkedProvider(null);
    if (
      active?.document.id === doc.id &&
      active.document.revision === doc.revision
    ) {
      if (!heading || !reader.current?.scrollToFragment(heading)) {
        const target = heading && parsed ? resolveHeading(parsed.outline, heading) : null;
        reader.current?.scrollToLine(target?.lineStart ?? line);
      }
    }
    setSyncSelection({ documentId: doc.id, sourceId: doc.source_id, revision: doc.revision, path: "document.md", title: doc.name });
    setActive({ document: doc, line, heading });
  };
  const incoming = useIncomingMarkdown(platform, navigationEpoch, {
    imported: refresh,
    open: (document) => open(document, 1),
  });
  const openReadingLink = useMobileReadingLinks(navigationEpoch, {
    openDevice: (document, line) => open(document, line),
    async openProvider(reference) {
      const epoch = ++navigationEpoch.current;
      await flushPosition();
      if (navigationEpoch.current !== epoch) return;
      setError(null); setActive(null); setLinkedProvider(reference);
    },
    failed: setError,
  });
  const transfer = async (reference: CanonicalReaderReference, action: 'export' | 'share') => {
    setTransferBusy(true);
    const images = assetImages;
    try {
      await images?.disposeAndDrain();
      return await transferDocument(reference, action);
    } finally { setTransferBusy(false); }
  };
  const close = async () => {
    const epoch = ++navigationEpoch.current;
    await flushPosition();
    if (navigationEpoch.current === epoch) setActive(null);
  };
  const openSyncSettings = async () => {
    const epoch = ++navigationEpoch.current;
    await flushPosition();
    if (navigationEpoch.current !== epoch) return;
    setActive(null); setTab("Settings"); setSyncFocus((value) => value + 1);
  };
  const saveAnchor = (anchor: ScrollAnchor) => {
    if (!active || !restoration.canSave()) return;
    const block = readerHost.current?.querySelector(
      `[data-block-index="${anchor.block}"] [data-line-start]`,
    );
    const line = Number(block?.getAttribute("data-line-start"));
    if (!Number.isSafeInteger(line) || line < 1) return;
    position.current = {
      id: active.document.id,
      revision: active.document.revision,
      line,
    };
    if (positionTimer.current !== null) clearTimeout(positionTimer.current);
    positionTimer.current = setTimeout(() => void flushPosition(), 300);
  };
  const readerPlatform = useMemo<Platform>(
    () => ({
      resolveAssetUrl: () => "about:blank",
      openExternal(url) {
        void openReaderExternal(url).catch((cause: unknown) => setError(message(cause)));
      },
    }),
    [],
  );
  const importFiles = async (files: FileList | null) => {
    if (!files?.length) return;
    setError(null);
    setNotice(null);
    const selected = Array.from(files);
    if (
      selected.length > 20 ||
      selected.some((file) => file.size > 1024 * 1024) ||
      selected.reduce((sum, file) => sum + file.size, 0) > 8 * 1024 * 1024
    ) {
      setError("Select up to 20 Markdown files, 1 MiB each and 8 MiB total.");
      return;
    }
    setBusy(true);
    let completed = 0;
    try {
      for (const file of selected) {
        const content = new TextDecoder("utf-8", {
          fatal: true,
          ignoreBOM: true,
        }).decode(await file.arrayBuffer());
        await result(commands.mobileReaderImport(file.name, content));
        completed++;
      }
    } catch (cause) {
      setError(
        `Imported ${completed} of ${selected.length} files. ${message(cause)}`,
      );
    } finally {
      try {
        await refresh();
      } catch (cause) {
        setError(message(cause));
      }
      setNotice(
        `${completed} file${completed === 1 ? "" : "s"} imported on this device.`,
      );
      setBusy(false);
    }
  };
  const togglePin = async () => {
    if (!active) return;
    try {
      const doc = active.document;
      await result(commands.mobileReaderPin(doc.id, !doc.pinned));
      setActive((current) =>
        current?.document.id === doc.id &&
        current.document.revision === doc.revision
          ? { ...current, document: { ...current.document, pinned: !doc.pinned } }
          : current,
      );
      await refresh();
    } catch (cause) {
      setError(message(cause));
    }
  };
  const library = (
    <section key="library" className="mobile-screen" aria-label="Library">
      <div className="mobile-screen__head">
        <span className="mobile-eyebrow">On this device</span>
        <h1>Your Markdown</h1>
      </div>
      <ReadingLinkOpen onOpen={openReadingLink} />
      <label className="mobile-touch">
        Import Markdown
        <input
          aria-label="Import Markdown"
          type="file"
          accept=".md,.markdown,text/markdown,text/x-markdown"
          multiple
          disabled={busy}
          onChange={(event) => {
            void importFiles(event.currentTarget.files);
            event.currentTarget.value = "";
          }}
        />
      </label>
      <p>
        Imported files stay on this device. Relative assets require a separate
        import; each file keeps its own identity.
      </p>
      {pending.length ? (
        <div className="mobile-state" aria-label="Interrupted imports">
          <p>Some imports were interrupted. Complete copies recover when you reopen the library; incomplete copies remain outside your library and use storage until removed. Your original files are preserved.</p>
          {pending.map((item) => (
            <div key={item.id}>
              <span>{item.name}</span>
              <button type="button" className="mobile-touch" disabled={busy} onClick={() => {
                setBusy(true);
                void result(commands.mobileReaderDiscardImport(item.id)).then(refresh)
                  .catch((cause) => setError(message(cause))).finally(() => setBusy(false));
              }}>Remove incomplete copy of {item.name}</button>
            </div>
          ))}
        </div>
      ) : null}
      {documents.some((doc) => !doc.indexed) ? (
        <div className="mobile-state">
          <p>
            Some imported documents need their search index rebuilt. Their
            original Markdown remains readable.
          </p>
          <button
            type="button"
            className="mobile-touch"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void result(commands.mobileReaderRebuild())
                .then(refresh)
                .catch((cause) => setError(message(cause)))
                .finally(() => setBusy(false));
            }}
          >
            Rebuild search index
          </button>
        </div>
      ) : null}
      {loading ? (
        <p role="status">Loading your library…</p>
      ) : documents.length === 0 ? (
        <p>No documents yet. Import Markdown to read offline.</p>
      ) : null}
      <ol className="mobile-document-list">
        {documents.map((doc) => (
          <li key={doc.id}>
            <button
              className="mobile-document"
              type="button"
              aria-label={doc.name}
              onClick={() => void open(doc)}
            >
              <span>
                <strong>{doc.name}</strong>
                <small>
                  {doc.pinned ? "Pinned · " : ""}
                  {Math.max(1, Math.ceil(doc.bytes / 1024))} KiB · Line{" "}
                  {doc.line}
                </small>
              </span>
            </button>
          </li>
        ))}
      </ol>
    </section>
  );
  return (
    <div
      className={`mobile-shell mobile-shell--device mobile-shell--${platform}`}
      data-mobile-platform={platform}
      data-system-theme={
        theme === "system" ? (systemDark ? "dark" : "light") : undefined
      }
    >
      {incoming.busy || incoming.error || incoming.notice ? (
        <div className="mobile-state" aria-label="Incoming Markdown">
          {incoming.busy ? <p role="status">Importing selected Markdown…</p> : null}
          {incoming.notice ? <p role="status">{incoming.notice}</p> : null}
          {incoming.error ? <p role="alert">{incoming.error}</p> : null}
          {incoming.retained ? <button type="button" className="mobile-touch" disabled={incoming.busy} onClick={incoming.retry}>Retry import</button> : null}
        </div>
      ) : null}
      {error ? (
        <p className="mobile-state" role="alert">
          {error}
        </p>
      ) : null}
      {notice ? (
        <p className="mobile-state" role="status">
          {notice}
        </p>
      ) : null}
      <div
        className={`mobile-shell__content${tablet ? " mobile-shell__content--tablet" : ""}`}
      >
        {active ? (
          <>
            {tablet ? (
              <div className="mobile-shell__library-pane">{library}</div>
            ) : null}
            <main
              className="mobile-reader"
              aria-label={evidence && activeAvailable ? "Document evidence" : "Reader"}
            >
              <header className="mobile-reader__bar">
                <button
                  className="mobile-touch"
                  type="button"
                  onClick={() => void close()}
                >
                  Back
                </button>
                <strong>{active.document.name}</strong>
                <button
                  className="mobile-touch"
                  type="button"
                  disabled={!activeAvailable}
                  onClick={() => void togglePin()}
                >
                  {active.document.pinned ? "Unpin" : "Pin"}
                </button>
                <button
                  className="mobile-touch"
                  type="button"
                  disabled={!activeAvailable}
                  onClick={() => setEvidence((value) => !value)}
                >
                  Evidence
                </button>
                <button
                  className="mobile-touch"
                  type="button"
                  disabled={!activeAvailable}
                  onClick={() => setOutline((value) => !value)}
                >
                  Outline
                </button>
                <button className="mobile-touch" type="button" aria-label="Sync this document"
                  disabled={!activeAvailable}
                  onClick={() => void openSyncSettings()}>Sync</button>
              </header>
              {transferReference && activeAvailable ? <DocumentActions reference={transferReference} line={active.line}
                includesSelectedImages nativeReadingLinks={transferSupport.readingLinks}
                nativeExport={transferSupport.export ? async (reference) => {
                  const state = await transfer(reference, 'export');
                  if (state === 'presented') throw new Error('Export could not be confirmed.');
                  return state;
                } : undefined}
                nativeShare={transferSupport.share ? async (reference) => {
                  const state = await transfer(reference, 'share');
                  if (state === 'exported') throw new Error('Sharing could not be confirmed.');
                  return state;
                } : undefined}
                getLine={() => position.current?.id === active.document.id && position.current.revision === active.document.revision ? position.current.line : active.line}
                read={async () => {
                  const value = await result(commands.mobileReaderRead(active.document.id, active.document.revision));
                  if (value.document.id !== active.document.id || value.document.source_id !== active.document.source_id || value.document.revision !== active.document.revision)
                    throw new Error("The exact device revision could not be verified.");
                  return { operation: "read_document", sourceId: value.document.source_id, path: "document.md", revision: value.document.revision, markdown: value.markdown, lineStart: 1, lineEnd: value.markdown.split("\n").length };
                }} /> : null}
              {evidence && activeAvailable ? (
                <section className="mobile-state">
                  <h2>Original imported Markdown</h2>
                  <dl>
                    <dt>Revision</dt>
                    <dd>{active.document.revision}</dd>
                    <dt>Source</dt>
                    <dd>{active.document.source_id}</dd>
                    <dt>Availability</dt>
                    <dd>
                      Durable on this device · searchable in Search
                    </dd>
                  </dl>
                </section>
              ) : null}
              {activeAvailable ? <DeviceAssetsSection key={`${active.document.id}:${active.document.revision}`} documentId={active.document.id}
                revision={active.document.revision} manifest={assetManifest} onRefresh={refreshAssets} /> : null}
              {activeAvailable && assetError ? <p className="mobile-state" role="alert">{assetError} <button type="button" onClick={refreshAssets}>Reload selected images</button></p> : null}
              {outline && parsed ? (
                <nav aria-label="Document outline">
                  {parsed.outline.map((item) => (
                    <button
                      key={item.slug}
                      type="button"
                      className="mobile-touch"
                      onClick={() =>
                        reader.current?.scrollToLine(item.lineStart)
                      }
                    >
                      {item.text}
                    </button>
                  ))}
                </nav>
              ) : null}
              <div ref={readerHost} className="mobile-reader__document"
                onPointerDownCapture={restoration.userIntent}
                onTouchStartCapture={restoration.userIntent}
                onWheelCapture={restoration.userIntent}
                onKeyDownCapture={restoration.userIntent}>
                {parsed ? (
                  <DeviceAssetProvider images={assetImages}>
                  <PlatformProvider platform={readerPlatform}>
                    <MarkdownReader
                      key={active.document.id}
                      ref={(handle) => {
                        if (restoration.attach(handle, readLine, activeHeading) === false)
                          setError("The requested location is unavailable in this revision. Showing the beginning of the document.");
                      }}
                      components={{ img: DeviceAssetImage }}
                      onScrollAnchorChange={saveAnchor}
                      onNavigate={(event) => {
                        if (event.kind === "anchor") {
                          if (!reader.current?.scrollToFragment(event.slug)) setError("The requested location is unavailable in this revision.");
                        } else {
                          const target = event.kind === "relative" ? event.href
                            : `${event.target}${event.heading ? `#${event.heading}` : ""}`;
                          const resolution = resolveImportedLink(documents, event.kind, target);
                          if (resolution.state === "resolved")
                            void open(resolution.document, 1, resolution.heading);
                          else setError(resolution.state === "ambiguous"
                            ? "This link is ambiguous. Open the intended file from Library."
                            : resolution.state === "unsupported"
                              ? "Imported files have no folder hierarchy. Open the selected file from Library."
                              : resolution.state === "invalid"
                                ? "This Markdown link is invalid."
                                : "Import the linked Markdown file before opening this link.");
                        }
                      }}
                    />
                  </PlatformProvider>
                  </DeviceAssetProvider>
                ) : readError ? (
                  <div className="mobile-state">
                    <p role="alert">{readError}</p>
                    <button type="button" className="mobile-touch" disabled={!activeAvailable} onClick={() => setReadAttempt((value) => value + 1)}>
                      Retry reading
                    </button>
                  </div>
                ) : (
                  <p role="status">Reading Markdown…</p>
                )}
              </div>
            </main>
          </>
        ) : linkedProvider ? (
          <main className="mobile-reader" aria-label="Cloud reader">
            <MobileProviderReader reference={linkedProvider} resumeSavedPlace={false} closeLabel={`Back to ${tab.toLowerCase()}`} onClose={() => {
              navigationEpoch.current++; setLinkedProvider(null);
            }} />
          </main>
        ) : tab === "Library" ? (
          library
        ) : tab === "Search" ? (
          <DeviceSearch session={searchSession.current} onOpen={(document, line) => void open(document, line)} />
        ) : tab === "Sources" ? (
          <MobileSources status={account.status} onSync={() => void openSyncSettings()} onAssetsChanged={refreshAssets} onSourcesChanged={() => void observeLibrary()} />
        ) : (
          <MobileSyncSettings account={account} selected={syncSelection} theme={theme}
            onTheme={setTheme} focusSync={syncFocus} />
        )}
      </div>
      {!active && !linkedProvider ? (
        <nav className="mobile-nav" aria-label="Primary">
          {(["Library", "Search", "Sources", "Settings"] as const).map(
            (name) => (
              <button
                key={name}
                type="button"
                className="mobile-nav__item"
                aria-current={name === tab ? "page" : undefined}
                onClick={() => {
                  navigationEpoch.current++;
                  setTab(name);
                  if (name === "Library") void refresh().catch((cause) => setError(message(cause)));
                }}
              >
                {name}
              </button>
            ),
          )}
        </nav>
      ) : null}
    </div>
  );
}
