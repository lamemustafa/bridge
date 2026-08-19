// SPDX-License-Identifier: Apache-2.0

import React from "react";

type Props = {
  children: React.ReactNode;
  // Shown in the fallback panel so an operator can tell which screen broke.
  label?: string;
};

type State = {
  error: Error | null;
};

/// Contains a render failure to the single panel that produced it. Before
/// this existed, one broken screen (e.g. a Rules-of-Hooks violation) unmounted
/// the entire React tree and left the app a blank white window with no
/// message -- the sidebar and every other screen went with it. Each mounted
/// screen gets its own instance, keyed on the active view, so switching away
/// from a broken screen and back retries the render instead of staying stuck.
export class ErrorBoundary extends React.Component<Props, State> {
  constructor(props: Props) {
    super(props);
    this.state = { error: null };
  }

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error(`ErrorBoundary caught a render failure${this.props.label ? ` in ${this.props.label}` : ""}:`, error, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <section className="panel wide error-boundary-panel" role="alert">
          <h2>{this.props.label ? `${this.props.label} hit a problem` : "This screen hit a problem"}</h2>
          <p>{this.state.error.message || "An unexpected error stopped this screen from rendering."}</p>
          <button type="button" onClick={() => window.location.reload()}>Reload</button>
        </section>
      );
    }
    return this.props.children;
  }
}
