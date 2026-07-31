'use client';

import { Component } from 'react';
import type { ReactNode } from 'react';

interface ErrorBoundaryProps {
  /** Rendered instead of the children when a render error is caught. */
  fallback: ReactNode;
  children: ReactNode;
}

interface ErrorBoundaryState {
  hasError: boolean;
}

/**
 * Minimal error boundary. Used to guarantee that a failing rendered subtree
 * (e.g. a bad Mermaid diagram) degrades to a safe fallback rather than tearing
 * down the whole reader.
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  constructor(props: ErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false };
  }

  static getDerivedStateFromError(): ErrorBoundaryState {
    return { hasError: true };
  }

  override render(): ReactNode {
    if (this.state.hasError) {
      return this.props.fallback;
    }
    return this.props.children;
  }
}
