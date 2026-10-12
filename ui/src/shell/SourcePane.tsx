/** Desktop Markdown source editor. The live instance retains cursor and undo
 * history across Edit/Preview/Split; preview never writes the buffer. */

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
import { defaultKeymap, history, historyKeymap, indentWithTab, undo, redo } from '@codemirror/commands';
import { markdown } from '@codemirror/lang-markdown';
import { syntaxHighlighting, HighlightStyle } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { search, searchKeymap, highlightSelectionMatches, openSearchPanel } from '@codemirror/search';
import { formatChord, formatTransaction, type FormatCommand } from './source-formatting';
import { textSizeChord } from './text-size-chord';
import { useSettingsStore, TEXT_SCALE_STEPS } from '../stores/settings';
import { useEditorStore } from '../stores/editor';

import { detectMac } from '../keymap/registry.js';
import { sourceChord } from './source-chord.js';
import { sourceLineSeparator, sourceText } from './source-text.js';
import { editorAdmission } from '../features/editor-preservation/admission';

// An external owner value is already in the store: reflecting it in the view
// must not echo a normalized representation back as a user edit.
const externalValue = Annotation.define<boolean>();

export interface SourcePaneCommands {
  format(command: FormatCommand): boolean;
  focus(): void;
}

export interface SourcePaneProps {
  /** Retain cursor when a canonical document's source view is remounted. */
  documentKey?: string;
  visible?: boolean;
  commandsRef?: { current: SourcePaneCommands | null };
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
  '.cm-panels': { fontSize: '13px', fontFamily: 'var(--font-ui)' },
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

export function SourcePane({ value, onChange, onSave, onToggleMode, onScrollLine, visible = true, commandsRef, documentKey }: SourcePaneProps) {
  const textScaleIndex = useSettingsStore((state) => state.textScaleIndex);
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
      keydown: (event, view) => {
        const size = textSizeChord(isMac, event);
        const format = formatChord(isMac, event);
        if (size || format) {
          event.preventDefault();
          event.stopPropagation();
          if (size) useSettingsStore.getState()[size === 'increase' ? 'increaseTextSize' : 'decreaseTextSize']();
          else if (format && !editorAdmission.denied()) view.dispatch(formatTransaction(view.state, format));
          return true;
        }
        const chord = sourceChord(isMac, event);
        if (chord && editorAdmission.denied()) { event.preventDefault(); event.stopPropagation(); return true; }
        if (chord === 'find' || chord === 'undo' || chord === 'redo') {
          event.preventDefault(); event.stopPropagation();
          if (chord === 'find') openSearchPanel(view);
          else if (chord === 'undo') undo(view);
          else redo(view);
          return true;
        }
        if (chord === 'save') {
          event.preventDefault();
          event.stopPropagation();
          onSaveRef.current();
          return true;
        }
        if (chord === 'toggle') {
          event.preventDefault();
          event.stopPropagation();
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
      EditorView.contentAttributes.of({ 'aria-label': 'Markdown source', 'aria-readonly': String(editorAdmission.denied()) }),
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
      search({ top: true }),
      highlightSelectionMatches(),
      EditorState.allowMultipleSelections.of(true),
      markdown(),
      syntaxHighlighting(mdHighlight),
      EditorView.lineWrapping,
      domHandlers,
      keymap.of([...searchKeymap, ...defaultKeymap, ...historyKeymap, indentWithTab]),
      readerTheme,
      EditorView.updateListener.of((update) => {
        if (update.docChanged && !update.transactions.some((transaction) => transaction.annotation(externalValue))) {
          onChangeRef.current(update.state.doc.sliceString(0, update.state.doc.length, separator.current));
        }
      }),
    ];

    const remembered = documentKey ? useEditorStore.getState().cursors[documentKey] : undefined;
    const doc = sourceText(value);
    const view = new EditorView({
      state: EditorState.create({ doc, extensions, ...(remembered ? { selection: {
        anchor: Math.min(remembered.anchor, doc.length), head: Math.min(remembered.head, doc.length),
      } } : {}) }),
      parent: hostRef.current,
    });
    if (remembered) view.scrollDOM.scrollTop = remembered.scrollTop;
    viewRef.current = view;
    const unsubscribe = editorAdmission.subscribe(() => {
      view.dispatch({ effects: admission.reconfigure(admissionExtensions()) });
    });
    return () => {
      unsubscribe();
      if (documentKey) useEditorStore.getState().rememberCursor(documentKey, {
        anchor: view.state.selection.main.anchor, head: view.state.selection.main.head, scrollTop: view.scrollDOM.scrollTop,
      });
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

  useEffect(() => {
    if (visible) viewRef.current?.requestMeasure();
  }, [visible, textScaleIndex]);

  useEffect(() => {
    if (!commandsRef) return;
    commandsRef.current = {
      focus: () => viewRef.current?.focus(),
      format: (command) => {
        const view = viewRef.current;
        if (!view || editorAdmission.denied()) return false;
        view.dispatch(formatTransaction(view.state, command));
        view.focus();
        return true;
      },
    };
    return () => { commandsRef.current = null; };
  }, [commandsRef]);

  return <div className="source-pane" ref={hostRef} style={{ '--text-0': `calc(1rem * ${TEXT_SCALE_STEPS[textScaleIndex] ?? 1})` } as React.CSSProperties} />;
}
