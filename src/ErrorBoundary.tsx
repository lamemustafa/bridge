// SPDX-License-Identifier: Apache-2.0

import React from "react";

let nextReloadAdmissionToken = 0;

export type ReloadGuard = {
  sourceDraftDirtyRef: React.RefObject<boolean>;
  sourceDraftActionBusyRef: React.RefObject<boolean>;
  journalActionBusyRef: React.RefObject<boolean>;
  lifecyclePendingRef: React.RefObject<boolean>;
  reloadAdmissionRef: React.RefObject<string | null>;
  authorizedReloadRef: React.RefObject<boolean>;
  inspectNativeLifecyclePending: () => Promise<boolean>;
};

export const ReloadGuardContext = React.createContext<ReloadGuard | null>(null);

type Props = {
  children: React.ReactNode;
  // Shown in the fallback panel so an operator can tell which screen broke.
  label?: string;
  onReload?: () => void;
};

type State = {
  error: Error | null;
  reloadConfirmation: boolean;
  reloadBlockedReason: string | null;
  reloadChecking: boolean;
};

/// Contains a render failure to the single panel that produced it. Before
/// this existed, one broken screen (e.g. a Rules-of-Hooks violation) unmounted
/// the entire React tree and left the app a blank white window with no
/// message -- the sidebar and every other screen went with it. Each mounted
/// screen gets its own instance, keyed on the active view, so switching away
/// from a broken screen and back retries the render instead of staying stuck.
export class ErrorBoundary extends React.Component<Props, State> {
  static contextType = ReloadGuardContext;
  declare context: React.ContextType<typeof ReloadGuardContext>;

  constructor(props: Props) {
    super(props);
    this.state = { error: null, reloadConfirmation: false, reloadBlockedReason: null, reloadChecking: false };
  }

  static getDerivedStateFromError(error: Error): State {
    return { error, reloadConfirmation: false, reloadBlockedReason: null, reloadChecking: false };
  }

  private reloadChecking = false;
  private reloading = false;
  private mounted = true;
  private readonly reloadAdmissionToken = `error-boundary-reload-${++nextReloadAdmissionToken}`;

  componentDidMount() {
    this.mounted = true;
  }

  componentWillUnmount() {
    this.mounted = false;
    if (!this.reloading && this.context?.reloadAdmissionRef.current === this.reloadAdmissionToken) {
      this.context.reloadAdmissionRef.current = null;
    }
  }

  private reload = (guard: ReloadGuard) => {
    this.reloading = true;
    guard.authorizedReloadRef.current = true;
    if (this.props.onReload) {
      this.props.onReload();
      guard.authorizedReloadRef.current = false;
      this.reloading = false;
      return;
    }
    try {
      window.location.reload();
    } catch (cause) {
      guard.authorizedReloadRef.current = false;
      this.reloading = false;
      throw cause;
    }
  };

  private reloadBlockReason(guard: ReloadGuard) {
    if (guard.lifecyclePendingRef.current) return "A native close request is still pending. Resolve it before reloading.";
    if (guard.journalActionBusyRef.current) return "A Journal action is still in progress. Wait for it to finish before reloading.";
    if (guard.sourceDraftActionBusyRef.current) return "A source-draft action is still in progress. Wait for it to finish before reloading.";
    return null;
  }

  private requestReload = () => {
    void this.admitReload(false);
  };

  private confirmReload = () => {
    void this.admitReload(true);
  };

  private async admitReload(discardDirty: boolean) {
    const guard = this.context;
    if (!guard) {
      window.location.reload();
      return;
    }
    const localBlock = this.reloadBlockReason(guard);
    if (localBlock) {
      this.setState({ reloadBlockedReason: localBlock });
      return;
    }
    if (this.reloadChecking) return;
    if (guard.reloadAdmissionRef.current && guard.reloadAdmissionRef.current !== this.reloadAdmissionToken) {
      this.setState({ reloadBlockedReason: "Another reload check is still in progress. Wait for it to finish." });
      return;
    }
    this.reloadChecking = true;
    guard.reloadAdmissionRef.current = this.reloadAdmissionToken;
    this.setState({ reloadChecking: true, reloadBlockedReason: null });
    try {
      const nativePending = await guard.inspectNativeLifecyclePending();
      if (!this.mounted) return;
      const afterCheckBlock = nativePending ? "A native close request is still pending. Resolve it before reloading." : this.reloadBlockReason(guard);
      if (afterCheckBlock) {
        this.setState({ reloadBlockedReason: afterCheckBlock, reloadConfirmation: false });
        return;
      }
      if (guard.sourceDraftDirtyRef.current && !discardDirty) {
        this.setState({ reloadConfirmation: true, reloadBlockedReason: null });
        return;
      }
      this.reload(guard);
    } catch {
      if (this.mounted) this.setState({ reloadBlockedReason: "Bridge could not confirm native close state. Resolve it before reloading.", reloadConfirmation: false });
    } finally {
      if (!this.reloading && guard.reloadAdmissionRef.current === this.reloadAdmissionToken) {
        guard.reloadAdmissionRef.current = null;
      }
      this.reloadChecking = false;
      if (this.mounted) this.setState({ reloadChecking: false });
    }
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
          {this.state.reloadBlockedReason && <p className="error-banner" role="alert">{this.state.reloadBlockedReason}</p>}
          {this.state.reloadConfirmation ? (
            <section className="source-draft-confirm" role="group" aria-labelledby="error-boundary-reload-heading">
              <h3 id="error-boundary-reload-heading">Discard unsaved proposals and reload?</h3>
              <p>Reloading discards local source-draft proposals. Keep this window open to preserve them.</p>
              <div className="journal-actions">
                <button type="button" onClick={this.confirmReload} disabled={this.state.reloadChecking}>Reload and discard</button>
                <button className="secondary-action" type="button" onClick={() => this.setState({ reloadConfirmation: false })} disabled={this.state.reloadChecking}>Keep current window</button>
              </div>
            </section>
          ) : <button type="button" onClick={this.requestReload} disabled={this.state.reloadChecking}>Reload</button>}
        </section>
      );
    }
    return this.props.children;
  }
}
