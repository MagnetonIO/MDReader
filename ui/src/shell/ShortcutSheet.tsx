/** The Cmd+/ shortcut sheet. Rendered straight from the one KeymapRegistry, so it
 * can never drift from the live bindings. */

export interface ShortcutGroup {
  group: string;
  items: { id: string; description: string; keys: string }[];
}

export interface ShortcutSheetProps {
  groups: ShortcutGroup[];
  onClose: () => void;
}

export function ShortcutSheet({ groups, onClose }: ShortcutSheetProps) {
  return (
    <div
      className="overlay-backdrop"
      role="presentation"
      onMouseDown={onClose}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onClose();
      }}
    >
      <div
        className="sheet"
        role="dialog"
        aria-label="Keyboard shortcuts"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <h2 className="sheet__title">Keyboard shortcuts</h2>
        <div className="sheet__grid">
          {groups.map((group) => (
            <section key={group.group}>
              <div className="sheet__group-title">{group.group}</div>
              {group.items.map((item) => (
                <div key={item.id} className="sheet__row">
                  <span>{item.description}</span>
                  <span className="kbd">{item.keys}</span>
                </div>
              ))}
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
