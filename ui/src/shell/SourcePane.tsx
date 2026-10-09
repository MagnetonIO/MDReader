/**
 * The light-editing source pane: a CodeMirror 6 editor for the active document's
 * markdown, used by the source and split view modes. Deliberately minimal — the
 * plan's posture is READ-FIRST, "a toggleable source pane for quick edits, NOT
 * an editor product" — so this wires just the essentials: markdown language,
 * history/undo, line numbers, and a theme that references the shared reading
 * tokens (so it recolors for light/dark via `[data-theme]` with no rebuild).
 *
 * This component (and therefore CodeMirror) lives in the DESKTOP app package,
 * never in the platform-neutral `@mdreader/ui` package.
 *
 * ## Split scroll sync (approximate, line-based)
 * In split mode the editor is the driver: as it scrolls we read the source line
 * at the top of the viewport and ask the preview to scroll that line into view
 * (`onScrollLine`). This is intentionally approximate — a markdown source line
 * does not map 1:1 to a rendered block height — but it keeps the two panes
 * roughly aligned without a brittle exact-mapping. Only the editor drives, to
 * avoid a two-way feedback loop.
 */

import { useEffect, useRef } from 'react';
import { Annotation, Compartment, EditorState, type Extension } from '@codemirror/state';
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
  drawSelection,
} from '@codemirror/view';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import { markdown } from '@codemirror/lang-markdown';
import { syntaxHighlighting, HighlightStyle } from '@codemirror/language';
import { tags } from '@lezer/highlight';

import { detectMac } from '../keymap/registry.js';
import { sourceChord } from './source-chord.js';
import { sourceLineSeparator, sourceText } from './source-text.js';
import { editorAdmission } from '../features/editor-preservation/admission';

// An external owner value is already in the store: reflecting it in the view
// must not echo a normalized representation back as a user edit.
const externalValue = Annotation.define<boolean>();

export interface SourcePaneProps {
  /** The document buffer to edit (a controlled value; external changes — reload,
   *  keep-mine — are reconciled into the live editor). */
  value: string;
  /** Fired on every user edit with the full new buffer. */
  onChange: (value: string) => void;
  /** Save chord (Cmd/Ctrl+S) — handled here too so it fires while the editor has
   *  focus (the global window handler skips editable targets). */
  onSave: () => void;
  /** Cmd/Ctrl+E from inside the editor — toggles back to reading view (the global
   *  handler can't see this keystroke while the editable editor holds focus). */
  onToggleMode: () => void;
  /** In split mode, the editor drives the preview: the top source line on scroll. */
  onScrollLine?: (line: number) => void;
}

/** Markdown token styling keyed to the reading tokens, so headings/links/etc.
 *  stay legible in both themes (the values are `var()` refs, theme-agnostic). */
const mdHighlight = HighlightStyle.define([
  { tag: tags.heading, fontWeight: '600', color: 'var(--color-fg)' },
  { tag: tags.strong, fontWeight: '700', color: 'var(--color-fg)' },
  { tag: tags.emphasis, fontStyle: 'italic' },
  { tag: [tags.link, tags.url], color: 'var(--color-accent)' },
  { tag: tags.monospace, color: 'var(--color-muted)' },
  { tag: tags.quote, color: 'var(--color-muted)', fontStyle: 'italic' },
  { tag: tags.list, color: 'var(--color-muted)' },
  {
    tag: [tags.processingInstruction, tags.meta, tags.comment],
    color: 'var(--color-faint)',
  },
]);

/** The editor chrome, wired to the shared tokens via `var()` so a theme flip on
 *  the document root recolors the editor with no extension rebuild. */
const readerTheme = EditorView.theme({
  '&': {
    color: 'var(--color-fg)',
    backgroundColor: 'var(--color-bg)',
    height: '100%',
    fontSize: 'var(--text-0)',
  },
  '.cm-scroller': {
    fontFamily: 'var(--font-mono)',
    lineHeight: '1.7',
    padding: 'var(--space-3) 0',
  },
  '.cm-content': { caretColor: 'var(--color-accent)' },
  '&.cm-focused .cm-cursor': { borderLeftColor: 'var(--color-accent)' },
  '&.cm-focused .cm-selectionBackground, ::selection': {
    backgroundColor: 'var(--color-selection)',
  },
  '.cm-selectionBackground': { backgroundColor: 'var(--color-selection)' },
  '.cm-gutters': {
    backgroundColor: 'var(--color-bg)',
    color: 'var(--color-faint)',
    border: 'none',
  },
  '.cm-activeLine': { backgroundColor: 'var(--color-surface)' },
  '.cm-activeLineGutter': {
    backgroundColor: 'var(--color-surface)',
    color: 'var(--color-muted)',
  },
});

/** The line (1-based) at the very top of the editor viewport — the anchor the
 *  split preview scrolls to. */
function topVisibleLine(view: EditorView): number {
  const block = view.lineBlockAtHeight(view.scrollDOM.scrollTop + view.documentTop);
  return view.state.doc.lineAt(block.from).number;
}

export function SourcePane({ value, onChange, onSave, onToggleMode, onScrollLine }: SourcePaneProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  // Keep CodeMirror's default mixed-newline input handling. Only serialization
  // uses the original convention, so pasting LF into CRLF remains normal input.
  const separator = useRef(sourceLineSeparator(value));
  // Latest callbacks, read through refs so the editor is built ONCE (rebuilding
  // it on every render would drop cursor/undo state and thrash CM6).
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const onSaveRef = useRef(onSave);
  onSaveRef.current = onSave;
  const onToggleModeRef = useRef(onToggleMode);
  onToggleModeRef.current = onToggleMode;
  const onScrollLineRef = useRef(onScrollLine);
  onScrollLineRef.current = onScrollLine;

  // Build the editor once, on mount. Subsequent external `value` changes are
  // reconciled by the effect below rather than rebuilding.
  useEffect(() => {
    if (!hostRef.current) return;

    // Save (Cmd/Ctrl+S) and mode-toggle (Cmd/Ctrl+E) are handled as raw DOM
    // keydowns rather than a CM6 keymap binding: the editor holds focus (the
    // global window handler skips editable targets), and a direct handler fires
    // reliably regardless of keymap precedence, preventing the browser's own
    // Cmd+S from ever reaching the page. The modifier is platform-checked: on
    // macOS ONLY ⌘ is reserved, so Ctrl+E stays CM6's native line-end (and Ctrl+S
    // is left alone); elsewhere the chord is Ctrl.
    const isMac = detectMac();
    const domHandlers = EditorView.domEventHandlers({
      scroll: (_event, view) => {
        if (onScrollLineRef.current) onScrollLineRef.current(topVisibleLine(view));
      },
      keydown: (event) => {
        const chord = sourceChord(isMac, event);
        if (chord && editorAdmission.denied()) { event.preventDefault(); return true; }
        if (chord === 'save') {
          event.preventDefault();
          onSaveRef.current();
          return true;
        }
        if (chord === 'toggle') {
          event.preventDefault();
          onToggleModeRef.current();
          return true;
        }
        return false;
      },
    });

    const admission = new Compartment();
    const admissionExtensions = () => [
      EditorState.readOnly.of(editorAdmission.denied()),
      EditorView.editable.of(!editorAdmission.denied()),
      EditorView.contentAttributes.of({ 'aria-readonly': String(editorAdmission.denied()) }),
    ];
    const extensions: Extension[] = [
      admission.of(admissionExtensions()),
      // Synchronous store admission also fences changes before a view effect or
      // React render could disable contenteditable (including paste/undo/IME).
      EditorState.transactionFilter.of((transaction) =>
        transaction.docChanged && editorAdmission.denied() ? [] : transaction),
      lineNumbers(),
      highlightActiveLine(),
      highlightActiveLineGutter(),
      drawSelection(),
      history(),
      EditorState.allowMultipleSelections.of(true),
      markdown(),
      syntaxHighlighting(mdHighlight),
      EditorView.lineWrapping,
      domHandlers,
      keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
      readerTheme,
      EditorView.updateListener.of((update) => {
        if (update.docChanged && !update.transactions.some((transaction) => transaction.annotation(externalValue))) {
          onChangeRef.current(update.state.doc.sliceString(0, update.state.doc.length, separator.current));
        }
      }),
    ];

    const view = new EditorView({
      state: EditorState.create({ doc: sourceText(value), extensions }),
      parent: hostRef.current,
    });
    viewRef.current = view;
    const unsubscribe = editorAdmission.subscribe(() => {
      view.dispatch({ effects: admission.reconfigure(admissionExtensions()) });
    });
    return () => {
      unsubscribe();
      view.destroy();
      viewRef.current = null;
    };
    // Built once (empty deps): `value` is reconciled by the next effect, and all
    // callbacks are read through refs, so the editor is never torn down + rebuilt.
  }, []);

  // Reconcile an EXTERNAL value change (reload from disk, keep-mine reset) into
  // the live editor — but never echo a value that already matches, so a user's
  // own keystroke (which flows out via onChange and back in as `value`) does not
  // reset their cursor.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const next = sourceText(value);
    separator.current = sourceLineSeparator(value);
    if (view.state.doc.eq(next)) return;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: next },
      annotations: externalValue.of(true),
    });
  }, [value]);

  return <div className="source-pane" ref={hostRef} />;
}
