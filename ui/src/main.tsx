import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

// Global stylesheets, tokens first so the shell + reader inherit the design
// system, then the reader's Shiki theme lane and KaTeX math styles.
import '@mdreader/config/tokens.css';
import '@mdreader/ui/styles.css';
import '@mdreader/markdown/katex.css';

import App from './App';

const rootElement = document.getElementById('root');
if (rootElement === null) {
  throw new Error('Root element #root not found');
}

createRoot(rootElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
