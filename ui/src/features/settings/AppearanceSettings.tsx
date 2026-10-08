import { textScaleFactor, useSettingsStore, TEXT_SCALE_STEPS } from '../../stores/settings.js';

export function AppearanceSettings() {
  const theme = useSettingsStore((state) => state.theme);
  const readingFont = useSettingsStore((state) => state.readingFont);
  const textScaleIndex = useSettingsStore((state) => state.textScaleIndex);
  return (<section className="settings-section" aria-labelledby="settings-appearance">
    <div className="settings-section__intro">
      <div>
        <h2 id="settings-appearance">Appearance</h2>
      </div>
      <p>Choose how MDReader looks and how documents are displayed.</p>
    </div>

    <div className="settings-field">
      <div className="settings-field__label">Theme</div>
      <div className="settings-segment" role="group" aria-label="Theme">
        {(['system', 'light', 'dark'] as const).map((value) => (
          <button
            key={value}
            type="button"
            aria-pressed={theme === value}
            onClick={() => useSettingsStore.getState().setTheme(value)}
          >
            {value[0]!.toUpperCase() + value.slice(1)}
          </button>
        ))}
      </div>
    </div>

    <div className="settings-field">
      <div>
        <div className="settings-field__label">Reading font</div>
        <div className="settings-field__hint">Only document typography changes.</div>
      </div>
      <div className="settings-segment" role="group" aria-label="Reading font">
        {(['serif', 'sans'] as const).map((value) => (
          <button
            key={value}
            type="button"
            aria-pressed={readingFont === value}
            onClick={() => useSettingsStore.getState().setReadingFont(value)}
          >
            {value[0]!.toUpperCase() + value.slice(1)}
          </button>
        ))}
      </div>
    </div>

    <div className="settings-field">
      <div>
        <div className="settings-field__label">Text size</div>
        <div className="settings-field__hint">
          {Math.round(textScaleFactor({ textScaleIndex }) * 100)}%
        </div>
      </div>
      <div className="settings-stepper">
        <button
          type="button"
          aria-label="Decrease text size"
          disabled={textScaleIndex === 0}
          onClick={() => useSettingsStore.getState().decreaseTextSize()}
        >
          −
        </button>
        <output aria-label="Text scale" aria-live="polite">{Math.round(textScaleFactor({ textScaleIndex }) * 100)}%</output>
        <button
          type="button"
          aria-label="Increase text size"
          disabled={textScaleIndex === TEXT_SCALE_STEPS.length - 1}
          onClick={() => useSettingsStore.getState().increaseTextSize()}
        >
          +
        </button>
      </div>
    </div>
    <figure className="settings-reading-preview" aria-label="Reading preview">
      <figcaption>Reading preview</figcaption>
      <div style={{ fontFamily: readingFont === 'serif' ? 'var(--font-reading-serif, Georgia, serif)' : 'var(--font-reading-sans, system-ui, sans-serif)', fontSize: `${16 * textScaleFactor({ textScaleIndex })}px` }}>
        <h3>A quiet place to read</h3>
        <p>Your Markdown, in a typeface and text size that feel comfortable.</p>
      </div>
    </figure>
    <p className="settings-footnote">Appearance changes apply immediately and are saved on this device.</p>
  </section>

  );
}
