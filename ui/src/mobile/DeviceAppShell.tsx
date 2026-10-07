import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { MarkdownReader, PlatformProvider } from "@mdreader/ui";
import type {
  MarkdownReaderHandle,
  Platform,
  ScrollAnchor,
} from "@mdreader/ui";
import { parseMarkdownAuto } from "@mdreader/markdown/worker";
import type { ParsedDoc } from "@mdreader/markdown";
import { commands } from "../bindings";
import type { DeviceDocument, DeviceHit, PendingDeviceImport } from "../bindings";
import { applyTheme, useSettingsStore } from "../stores/settings";
import { loadPersisted, saveSettings } from "../shell/persistence";
import "./mobile.css";

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

function ConsentedImage({ src, alt = "" }: { src?: string; alt?: string }) {
  const [approvedUrl, setApprovedUrl] = useState<string | null>(null);
  let safe = false;
  try {
    const url = new URL(src ?? "");
    safe = url.protocol === "https:" && !url.username && !url.password;
  } catch {
    /* relative assets need an explicit imported-asset grant */
  }
  if (safe && approvedUrl === src)
    return (
      <img
        src={src}
        alt={alt}
        loading="lazy"
        draggable={false}
        style={{ maxWidth: "100%" }}
      />
    );
  return (
    <span className="mobile-state">
      {alt || "Image"} —{" "}
      {safe ? (
        <button type="button" onClick={() => setApprovedUrl(src ?? null)}>
          Load remote image
        </button>
      ) : (
        "asset is not imported"
      )}
    </span>
  );
}

/** Account-free mobile reading over the embedded SQLite owner. */
export function DeviceAppShell({ platform }: { platform: "ios" | "android" }) {
  const [tab, setTab] = useState<Tab>("Library");
  const [documents, setDocuments] = useState<DeviceDocument[]>([]);
  const [pending, setPending] = useState<PendingDeviceImport[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<DeviceHit[]>([]);
  const [searching, setSearching] = useState(false);
  const [active, setActive] = useState<{
    document: DeviceDocument;
    line: number;
  } | null>(null);
  const [parsed, setParsed] = useState<ParsedDoc | null>(null);
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
  const searchEpoch = useRef(0);
  const reader = useRef<MarkdownReaderHandle | null>(null);
  const readerHost = useRef<HTMLDivElement | null>(null);
  const position = useRef<{
    id: string;
    revision: string;
    line: number;
  } | null>(null);
  const positionTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const restored = useRef(false);

  const refresh = useCallback(async () => {
    const value = await result(commands.mobileReaderList());
    setDocuments(value.documents);
    setPending(value.pending);
  }, []);
  const activeId = active?.document.id;
  const activeRevision = active?.document.revision;
  useEffect(() => {
    let current = true;
    void result(commands.mobileReaderList())
      .then((value) => {
        if (current) {
          setDocuments(value.documents);
          setPending(value.pending);
        }
      })
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
      searchEpoch.current++;
    };
  }, []);
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
          doc.id === pending.id ? { ...doc, line: pending.line } : doc,
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
    setParsed(null);
    restored.current = false;
    setEvidence(false);
    setOutline(false);
    if (!activeId || !activeRevision) return;
    void result(commands.mobileReaderRead(activeId, activeRevision))
      .then(async (value) => {
        const doc = await parseMarkdownAuto(value.markdown);
        if (epoch === readEpoch.current) setParsed(doc);
      })
      .catch((cause) => {
        if (epoch === readEpoch.current) setError(message(cause));
      });
    return () => {
      readEpoch.current++;
    };
  }, [activeId, activeRevision]);
  useEffect(() => {
    const epoch = ++searchEpoch.current;
    setHits([]);
    if (!query.trim()) {
      setSearching(false);
      return;
    }
    setSearching(true);
    const timer = setTimeout(() => {
      void result(commands.mobileReaderSearch(query))
        .then((value) => {
          if (epoch === searchEpoch.current) setHits(value);
        })
        .catch((cause) => {
          if (epoch === searchEpoch.current) setError(message(cause));
        })
        .finally(() => {
          if (epoch === searchEpoch.current) setSearching(false);
        });
    }, 150);
    return () => {
      clearTimeout(timer);
      searchEpoch.current++;
    };
  }, [query]);
  const open = async (doc: DeviceDocument, line = doc.line) => {
    await flushPosition();
    setError(null);
    if (
      active?.document.id === doc.id &&
      active.document.revision === doc.revision
    ) {
      reader.current?.scrollToLine(line);
    }
    setActive({ document: doc, line });
  };
  const close = async () => {
    await flushPosition();
    setActive(null);
  };
  const saveAnchor = (anchor: ScrollAnchor) => {
    if (!active || !restored.current) return;
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
        if (/^(https:|mailto:|tel:)/i.test(url))
          window.open(url, "_blank", "noopener,noreferrer");
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
      <label className="mobile-touch">
        Import Markdown
        <input
          aria-label="Import Markdown"
          type="file"
          accept=".md,text/markdown"
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
              aria-label={evidence ? "Document evidence" : "Reader"}
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
                  onClick={() => void togglePin()}
                >
                  {active.document.pinned ? "Unpin" : "Pin"}
                </button>
                <button
                  className="mobile-touch"
                  type="button"
                  onClick={() => setEvidence((value) => !value)}
                >
                  Evidence
                </button>
                <button
                  className="mobile-touch"
                  type="button"
                  onClick={() => setOutline((value) => !value)}
                >
                  Outline
                </button>
              </header>
              {evidence ? (
                <section className="mobile-state">
                  <h2>Original imported Markdown</h2>
                  <dl>
                    <dt>Revision</dt>
                    <dd>{active.document.revision}</dd>
                    <dt>Source</dt>
                    <dd>{active.document.source_id}</dd>
                    <dt>Availability</dt>
                    <dd>
                      Durable on this device · lexical search · semantic model
                      unavailable
                    </dd>
                  </dl>
                </section>
              ) : null}
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
              <div ref={readerHost} className="mobile-reader__document">
                {parsed ? (
                  <PlatformProvider platform={readerPlatform}>
                    <MarkdownReader
                      key={active.document.id}
                      ref={(handle) => {
                        reader.current = handle;
                        if (handle && !restored.current) {
                          handle.scrollToLine(active.line);
                          restored.current = true;
                        }
                      }}
                      doc={parsed}
                      components={{ img: ConsentedImage }}
                      onScrollAnchorChange={saveAnchor}
                      onNavigate={(event) => {
                        if (event.kind === "anchor") {
                          const target = parsed.outline.find(
                            (item) => item.slug === event.slug,
                          );
                          if (target)
                            reader.current?.scrollToLine(target.lineStart);
                        } else {
                          const target =
                            event.kind === "relative"
                              ? event.href
                              : event.target;
                          const matches = documents.filter(
                            (doc) => doc.name === target.split("#")[0],
                          );
                          if (matches.length === 1) void open(matches[0]!);
                          else
                            setError(
                              matches.length
                                ? "This link is ambiguous. Open the intended file from Library."
                                : "Import the linked Markdown file before opening this link.",
                            );
                        }
                      }}
                    />
                  </PlatformProvider>
                ) : (
                  <p role="status">Reading Markdown…</p>
                )}
              </div>
            </main>
          </>
        ) : tab === "Library" ? (
          library
        ) : tab === "Search" ? (
          <section key="search" className="mobile-screen" aria-label="Search">
            <h1>Search your Markdown</h1>
            <label className="mobile-field">
              Search
              <input
                type="search"
                placeholder="Words in your documents"
                value={query}
                onChange={(event) => setQuery(event.currentTarget.value)}
                maxLength={1024}
              />
            </label>
            <p>
              Full-text search on this device. Semantic search is unavailable on
              this build.
            </p>
            {searching ? (
              <p role="status">Searching…</p>
            ) : query && hits.length === 0 ? (
              <p>No matching passages.</p>
            ) : null}
            <ol className="mobile-document-list">
              {hits.map((hit) => (
                <li key={`${hit.document.id}:${hit.line}`}>
                  <button
                    type="button"
                    className="mobile-document"
                    aria-label={hit.document.name}
                    onClick={() => void open(hit.document, hit.line)}
                  >
                    <span>
                      <strong>
                        {hit.document.name} · Line {hit.line}
                      </strong>
                      <small>{hit.text}</small>
                    </span>
                  </button>
                </li>
              ))}
            </ol>
          </section>
        ) : tab === "Sources" ? (
          <section className="mobile-screen" aria-label="Sources">
            <h1>Sources</h1>
            <article className="mobile-source">
              <h2>Imported Markdown</h2>
              <p>
                Original files and reading state are durable on this device.
                Imports do not grant access to device folders.
              </p>
            </article>
            {["Google Drive", "Dropbox", "Encrypted cloud sync"].map((name) => (
              <article key={name} className="mobile-source">
                <h2>{name}</h2>
                <p>
                  Unavailable until mobile secure sign-in and transfer are
                  connected.
                </p>
              </article>
            ))}
          </section>
        ) : (
          <section className="mobile-screen" aria-label="Settings">
            <h1>Settings</h1>
            <fieldset className="mobile-theme">
              <legend>Appearance</legend>
              {(["system", "light", "dark"] as const).map((choice) => (
                <label key={choice}>
                  <input
                    type="radio"
                    name="mobile-theme"
                    checked={theme === choice}
                    onChange={() => setTheme(choice)}
                  />
                  {choice[0]!.toUpperCase() + choice.slice(1)}
                </label>
              ))}
            </fieldset>
            <h2>Offline copies</h2>
            <p>
              Imports and pins are never automatically evicted. This device
              library allows 1,000 documents or 64 MiB.
            </p>
          </section>
        )}
      </div>
      {!active ? (
        <nav className="mobile-nav" aria-label="Primary">
          {(["Library", "Search", "Sources", "Settings"] as const).map(
            (name) => (
              <button
                key={name}
                type="button"
                className="mobile-nav__item"
                aria-current={name === tab ? "page" : undefined}
                onClick={() => setTab(name)}
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
