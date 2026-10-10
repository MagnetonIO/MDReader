import { parseMarkdown } from '@mdreader/markdown';
import { MarkdownReader } from '@mdreader/ui';
import type { MarkdownReaderHandle } from '@mdreader/ui';
import { useEffect, useRef } from 'react';
import { MobileReaderFrame } from './MobileReaderFrame';
import { readingAppearance } from '../features/reader/appearance';
import { useAppearanceChange } from '../features/reader/useAppearanceChange';
import { useSettingsStore, textScaleFactor } from '../stores/settings';
import './mobile.css';

const doc = parseMarkdown('# Public reading example\n\nA reader keeps the document in view and puts secondary controls within reach.\n\n## A useful next section\n\nThis fixture contains no private library or provider data.');
function Example({ dark = false, enlarged = false }: { dark?: boolean; enlarged?: boolean }) {
  const reader = useRef<MarkdownReaderHandle>(null);
  const font = useSettingsStore(state => state.readingFont);
  const size = useSettingsStore(state => state.textScaleIndex);
  const changeAppearance = useAppearanceChange(reader, 'public-example');
  useEffect(() => {
    useSettingsStore.getState().hydrate({ readingFont: 'serif', textScaleIndex: enlarged ? 6 : 3 });
    return () => useSettingsStore.getState().reset();
  }, [enlarged]);
  return <div className={`mobile-shell mobile-shell--device mobile-shell--${enlarged ? 'android' : 'ios'}`}
    data-system-theme={dark ? 'dark' : 'light'} style={{ width: 320, height: 640, fontSize: enlarged ? 24 : 16 }}>
    <MobileReaderFrame identity="public-example" title="A long public document title that must fit a narrow phone.md" onClose={() => {}}
      actions={<><button type="button">Pin</button><button type="button">Share file</button><button type="button">Export</button></>}
      details={<p>Public presentation example.</p>} outline={doc.outline} onOutline={line => reader.current?.scrollToLine(line)} onAppearanceChange={changeAppearance}>
      <div style={{ ...readingAppearance(font, textScaleFactor({ textScaleIndex: size })), flex: 1, minHeight: 0 }}><MarkdownReader doc={doc} ref={reader} /></div>
    </MobileReaderFrame>
  </div>;
}
export const LightPhone320 = () => <Example />;
export const DarkPhone320 = () => <Example dark />;
export const EnlargedAndroidText320 = () => <Example dark enlarged />;
