import { Component, StrictMode, type ErrorInfo, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';

// Global stylesheets, tokens first so the shell + reader inherit the design
// system, then the reader's Shiki theme lane and KaTeX math styles.
import '@mdreader/config/tokens.css';
import '@mdreader/ui/styles.css';
import '@mdreader/markdown/katex.css';

import App from './App';
import { ReaderBackendProvider, readerBackend } from './platform/backend';

interface AppErrorBoundaryState {
  message: string | null;
}

class AppErrorBoundary extends Component<{ children: ReactNode }, AppErrorBoundaryState> {
  override state: AppErrorBoundaryState = { message: null };

  static getDerivedStateFromError(error: unknown): AppErrorBoundaryState {
    return {
      message: error instanceof Error ? error.message : 'The desktop renderer stopped unexpectedly.',
    };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error('MDReader renderer failure', error, info.componentStack);
  }

  override render(): ReactNode {
    if (this.state.message !== null) {
      return (
        <main className="app-failure" role="alert">
          <h1>MDReader could not display this document.</h1>
          <p>{this.state.message}</p>
          <button type="button" onClick={() => globalThis.location.reload()}>
            Reload MDReader
          </button>
        </main>
      );
    }
    return this.props.children;
  }
}

const rootElement = document.getElementById('root');
if (rootElement === null) {
  throw new Error('Root element #root not found');
}

createRoot(rootElement).render(
  <StrictMode>
    <AppErrorBoundary>
      <ReaderBackendProvider backend={readerBackend()}>
        <App />
      </ReaderBackendProvider>
    </AppErrorBoundary>
  </StrictMode>,
);
