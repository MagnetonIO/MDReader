/**
 * The tab strip. dnd-kit provides pointer + keyboard-accessible reordering;
 * middle-click closes; double-click pins; the close affordance appears on hover
 * or when active. Only the active tab's document is ever mounted (enforced by the
 * ReaderPane), so this strip is purely a selector over `useTabsStore`.
 */

import {
  DndContext,
  KeyboardSensor,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import type { DragEndEvent } from '@dnd-kit/core';
import {
  SortableContext,
  horizontalListSortingStrategy,
  sortableKeyboardCoordinates,
  useSortable,
} from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';

import { CloseIcon, PinIcon } from './icons.js';
import { useTabsStore } from '../stores/tabs.js';
import { useEditorStore } from '../stores/editor.js';
import { isLocalDocument, type TabState } from '../stores/types.js';

function SortableTab({
  tab,
  active,
  onLocalIntent,
  onRequestClose,
}: {
  tab: TabState;
  active: boolean;
  onLocalIntent: () => void;
  onRequestClose: (tabId: string) => void;
}) {
  const setActiveTab = useTabsStore((state) => state.setActiveTab);
  const togglePin = useTabsStore((state) => state.togglePin);
  // Unsaved-edit marker: a dot on the tab while ANY of its documents' buffers
  // diverge from disk (including a doc the tab has navigated away from). Sourced
  // from the editor store (the single source of truth for dirty).
  const dirty = useEditorStore((state) => state.tabIsDirty(tab.id));

  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({
    id: tab.id,
  });

  return (
    <div
      ref={setNodeRef}
      className={[
        'tab',
        active ? 'tab--active' : '',
        tab.pinned ? 'tab--pinned' : '',
        isDragging ? 'tab--dragging' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      style={{ transform: CSS.Transform.toString(transform), transition }}
      {...attributes}
      {...listeners}
      role="tab"
      aria-selected={active}
      title={isLocalDocument(tab.docRef) ? tab.docRef.path : tab.docRef.name}
      onClick={() => {
        onLocalIntent();
        setActiveTab(tab.id);
      }}
      onDoubleClick={() => togglePin(tab.id)}
      onAuxClick={(event) => {
        if (event.button === 1) {
          event.preventDefault();
          onLocalIntent();
          onRequestClose(tab.id);
        }
      }}
    >
      {tab.pinned ? <PinIcon className="tab__pin" /> : null}
      <span className="tab__label">{tab.docRef.name}</span>
      {dirty ? (
        <span className="tab__dirty" role="img" aria-label="Unsaved changes" title="Unsaved changes" />
      ) : null}
      <button
        type="button"
        className="tab__close"
        aria-label={`Close ${tab.docRef.name}`}
        onPointerDown={(event) => event.stopPropagation()}
        onClick={(event) => {
          event.stopPropagation();
          onLocalIntent();
          onRequestClose(tab.id);
        }}
      >
        <CloseIcon width="12" height="12" />
      </button>
    </div>
  );
}

export interface TabStripProps {
  /** Retire any non-tab reader before a local tab action takes ownership. */
  onLocalIntent: () => void;
  /** Request to close a tab — routed through the shell so a dirty tab prompts the
   *  unsaved-changes guard before it is torn down. */
  onRequestClose: (tabId: string) => void;
}

export function TabStrip({ onLocalIntent, onRequestClose }: TabStripProps) {
  const tabs = useTabsStore((state) => state.tabs);
  const activeTabId = useTabsStore((state) => state.activeTabId);
  const reorderTabs = useTabsStore((state) => state.reorderTabs);

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  function onDragEnd(event: DragEndEvent) {
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const from = tabs.findIndex((tab) => tab.id === active.id);
    const to = tabs.findIndex((tab) => tab.id === over.id);
    if (from >= 0 && to >= 0) reorderTabs(from, to);
  }

  if (tabs.length === 0) {
    return (
      <div className="tabstrip" role="tablist">
        <span className="tabstrip__empty">No open documents</span>
      </div>
    );
  }

  return (
    <div className="tabstrip" role="tablist" aria-label="Open documents">
      <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
        <SortableContext
          items={tabs.map((tab) => tab.id)}
          strategy={horizontalListSortingStrategy}
        >
          {tabs.map((tab) => (
            <SortableTab
              key={tab.id}
              tab={tab}
              active={tab.id === activeTabId}
              onLocalIntent={onLocalIntent}
              onRequestClose={onRequestClose}
            />
          ))}
        </SortableContext>
      </DndContext>
    </div>
  );
}
