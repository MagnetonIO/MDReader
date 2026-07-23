/** E2E entry: identical to the production entry but with the Tauri IPC mocks
 * installed before the shell renders. Served via `e2e.html` in dev only. */

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import '@mdreader/config/tokens.css';
import '@mdreader/ui/styles.css';
import '@mdreader/markdown/katex.css';

import App from '../App';
import { installTauriMocks } from './harness';

installTauriMocks();

const rootElement = document.getElementById('root');
if (rootElement === null) {
  throw new Error('Root element #root not found');
}

createRoot(rootElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
