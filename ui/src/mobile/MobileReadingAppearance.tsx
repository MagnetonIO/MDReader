import { TEXT_SCALE_STEPS, textScaleFactor, useSettingsStore } from '../stores/settings';

/** One persisted appearance vocabulary for imported and cloud Markdown. */
export function MobileReadingAppearance({ onChange = change => change() }: { onChange?: (change: () => void) => void }) {
  const font = useSettingsStore(state => state.readingFont);
  const size = useSettingsStore(state => state.textScaleIndex);
  return <fieldset className="mobile-reading-appearance">
    <legend>Reading appearance</legend>
    <div className="mobile-reading-appearance__fonts">
      {(['serif', 'sans'] as const).map(value => <label key={value}>
        <input type="radio" name="reading-font" checked={font === value}
          onChange={() => onChange(() => useSettingsStore.getState().setReadingFont(value))} />
        {value === 'serif' ? 'Serif' : 'Sans serif'}
      </label>)}
    </div>
    <div className="mobile-reading-appearance__size" role="group" aria-label="Reading text size">
      <button type="button" aria-label="Smaller text" disabled={size === 0}
        onClick={() => onChange(() => useSettingsStore.getState().decreaseTextSize())}>A−</button>
      <output aria-label="Text size">{Math.round(textScaleFactor({ textScaleIndex: size }) * 100)}%</output>
      <button type="button" aria-label="Larger text" disabled={size === TEXT_SCALE_STEPS.length - 1}
        onClick={() => onChange(() => useSettingsStore.getState().increaseTextSize())}>A+</button>
    </div>
  </fieldset>;
}
