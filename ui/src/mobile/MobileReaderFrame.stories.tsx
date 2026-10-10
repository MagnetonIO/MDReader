import { parseMarkdown } from '@mdreader/markdown';
import { MarkdownReader } from '@mdreader/ui';
import { MobileReaderFrame } from './MobileReaderFrame';
import { readingAppearance } from '../features/reader/appearance';
import './mobile.css';

const doc = parseMarkdown('# Public reading example\n\nA reader keeps the document in view and puts secondary controls within reach.\n\n## A useful next section\n\nThis fixture contains no private library or provider data.');
function Example({ dark = false, enlarged = false }: { dark?: boolean; enlarged?: boolean }) {
  return <div className={`mobile-shell mobile-shell--device mobile-shell--${enlarged ? 'android' : 'ios'}`}
    data-system-theme={dark ? 'dark' : 'light'} style={{ width: 320, height: 640, fontSize: enlarged ? 24 : 16 }}>
    <MobileReaderFrame identity="public-example" title="A long public document title that must fit a narrow phone.md" onClose={() => {}}
      actions={<><button type="button">Pin</button><button type="button">Share file</button><button type="button">Export</button></>}
      details={<p>Public presentation example.</p>} outline={doc.outline} onOutline={() => {}} onAppearanceChange={change => change()}>
      <div style={{ ...readingAppearance('serif', enlarged ? 1.3 : 1), flex: 1, minHeight: 0 }}><MarkdownReader doc={doc} /></div>
    </MobileReaderFrame>
  </div>;
}
export const LightPhone320 = () => <Example />;
export const DarkPhone320 = () => <Example dark />;
export const EnlargedAndroidText320 = () => <Example dark enlarged />;
