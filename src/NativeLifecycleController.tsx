import React from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createDrawerFocusLifecycle, ensureDrawerFocus, trapDrawerTabKeydown } from "./evidence-drawer-focus";
import "./source-draft.css";

export type NativeLifecycleKind = "close" | "exit";

export type NativeLifecycleRequest = {
  request_id: string;
  kind: NativeLifecycleKind;
};

export function hasNativeWindowRuntime() {
  const internals = (window as unknown as {
    __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: unknown } } };
  }).__TAURI_INTERNALS__;
  return typeof internals?.metadata?.currentWindow?.label === "string";
}

function sameRequest(left: NativeLifecycleRequest, right: NativeLifecycleRequest) {
  return left.request_id === right.request_id && left.kind === right.kind;
}

function errorMessage(cause: unknown) {
  if (cause instanceof Error) return cause.message;
  if (typeof cause === "string") return cause;
  if (cause && typeof cause === "object" && "message" in cause && typeof cause.message === "string") return cause.message;
  return "Bridge could not complete the native close request.";
}

type Props = {
  sourceDraftDirtyRef: React.RefObject<boolean>;
  sourceDraftActionBusyRef: React.RefObject<boolean>;
  journalActionBusyRef: React.RefObject<boolean>;
  lifecyclePendingRef: React.RefObject<boolean>;
  onProtectionChange: (ready: boolean, error: string | null) => void;
  onModalChange: (open: boolean) => void;
  onModalClosed: (restoreFocus: () => void) => void;
};

export function NativeLifecycleController({
  sourceDraftDirtyRef,
  sourceDraftActionBusyRef,
  journalActionBusyRef,
  lifecyclePendingRef,
  onProtectionChange,
  onModalChange,
  onModalClosed,
}: Props) {
  const [request, setRequest] = React.useState<NativeLifecycleRequest | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [requestError, setRequestError] = React.useState<string | null>(null);
  const mounted = React.useRef(true);
  const epoch = React.useRef(0);
  const requestRef = React.useRef<NativeLifecycleRequest | null>(null);
  const busyRef = React.useRef(false);
  const queuedRequestRef = React.useRef<NativeLifecycleRequest | null>(null);
  const handleRequestRef = React.useRef<((request: NativeLifecycleRequest) => void) | undefined>(undefined);
  const dialogRef = React.useRef<HTMLElement | null>(null);
  const dialogWasOpen = React.useRef(false);
  const focusLifecycle = React.useRef(createDrawerFocusLifecycle()).current;

  const setCurrentRequest = React.useCallback((next: NativeLifecycleRequest | null) => {
    requestRef.current = next;
    lifecyclePendingRef.current = next !== null;
    setRequest(next);
  }, [lifecyclePendingRef]);

  const completionBlocked = React.useCallback(
    () => sourceDraftActionBusyRef.current || journalActionBusyRef.current,
    [journalActionBusyRef, sourceDraftActionBusyRef],
  );

  const complete = React.useCallback(async (next = requestRef.current) => {
    if (!next || busyRef.current || completionBlocked()) return;
    epoch.current += 1;
    busyRef.current = true;
    setBusy(true);
    setRequestError(null);
    try {
      await invoke("desktop_complete_source_draft_lifecycle_request", { request: next });
    } catch (cause) {
      if (mounted.current) setRequestError(errorMessage(cause));
    } finally {
      busyRef.current = false;
      if (mounted.current) setBusy(false);
      const queued = queuedRequestRef.current;
      queuedRequestRef.current = null;
      if (mounted.current && queued) void handleRequestRef.current?.(queued);
    }
  }, [completionBlocked]);

  const cancel = React.useCallback(async () => {
    const next = requestRef.current;
    if (!next || busyRef.current) return;
    epoch.current += 1;
    busyRef.current = true;
    setBusy(true);
    setRequestError(null);
    try {
      await invoke("desktop_cancel_source_draft_lifecycle_request", { request: next });
      if (mounted.current && sameRequest(requestRef.current ?? next, next)) {
        const preservePendingAdmission = queuedRequestRef.current !== null;
        requestRef.current = null;
        lifecyclePendingRef.current = preservePendingAdmission;
        setRequest(null);
      }
    } catch (cause) {
      if (mounted.current) setRequestError(errorMessage(cause));
    } finally {
      busyRef.current = false;
      if (mounted.current) setBusy(false);
      const queued = queuedRequestRef.current;
      queuedRequestRef.current = null;
      if (mounted.current && queued) void handleRequestRef.current?.(queued);
    }
  }, [lifecyclePendingRef]);

  React.useLayoutEffect(() => {
    const open = request !== null;
    onModalChange(open);
    if (open) {
      if (!dialogWasOpen.current) {
        focusLifecycle.captureOpener(document.activeElement instanceof HTMLElement ? document.activeElement : null);
        dialogWasOpen.current = true;
      }
      ensureDrawerFocus(true, dialogRef.current);
    } else if (dialogWasOpen.current) {
      dialogWasOpen.current = false;
      onModalClosed(() => {
        focusLifecycle.restoreOpener();
      });
    }
  }, [focusLifecycle, onModalChange, onModalClosed, request]);

  React.useLayoutEffect(() => () => {
    onModalChange(false);
  }, [onModalChange]);

  React.useEffect(() => {
    mounted.current = true;
    if (!hasNativeWindowRuntime()) {
      onProtectionChange(true, null);
      return () => {
        mounted.current = false;
      };
    }

    let active = true;
    let unlisten: (() => void) | undefined;
    let registered = false;
    const registrationToken = crypto.randomUUID();

    const handleRequest = async (next: NativeLifecycleRequest) => {
      if (!active) return;
      // Native requested a close before this lookup can settle. Block local
      // admission immediately; a same-epoch negative lookup may clear it.
      lifecyclePendingRef.current = true;
      if (busyRef.current) {
        if (!requestRef.current || !sameRequest(requestRef.current, next)) {
          const queued = queuedRequestRef.current;
          if (!queued || !sameRequest(queued, next)) queuedRequestRef.current = next;
        }
        return;
      }
      const currentEpoch = ++epoch.current;
      let pending: NativeLifecycleRequest | null;
      try {
        pending = await invoke<NativeLifecycleRequest | null>("desktop_pending_source_draft_lifecycle_request");
      } catch (cause) {
        if (active && currentEpoch === epoch.current) {
          setCurrentRequest(next);
          setRequestError(`Bridge could not inspect this native close request: ${errorMessage(cause)}`);
        }
        return;
      }
      if (!active || currentEpoch !== epoch.current) return;
      if (!pending) {
        if (requestRef.current === null) lifecyclePendingRef.current = false;
        return;
      }
      if (!sameRequest(pending, next)) {
        setRequestError(null);
        setCurrentRequest(pending);
        if (sourceDraftDirtyRef.current || completionBlocked()) return;
        void complete(pending);
        return;
      }
      setRequestError(null);
      setCurrentRequest(next);
      if (sourceDraftDirtyRef.current || completionBlocked()) return;
      void complete(next);
    };
    handleRequestRef.current = handleRequest;

    void (async () => {
      try {
        const registeredUnlisten = await listen<NativeLifecycleRequest>("source-draft-lifecycle-requested", (event) => {
          void handleRequest(event.payload);
        });
        if (!active) {
          registeredUnlisten();
          return;
        }
        unlisten = registeredUnlisten;
        await invoke("desktop_register_source_draft_lifecycle_renderer", { token: registrationToken });
        if (!active) {
          void invoke("desktop_unregister_source_draft_lifecycle_renderer", { token: registrationToken });
          return;
        }
        registered = true;
        onProtectionChange(true, null);
        const pending = await invoke<NativeLifecycleRequest | null>("desktop_pending_source_draft_lifecycle_request");
        if (active && pending) await handleRequest(pending);
      } catch (cause) {
        if (active) onProtectionChange(false, `Bridge could not install native close protection: ${errorMessage(cause)}`);
      }
    })();

    return () => {
      active = false;
      mounted.current = false;
      handleRequestRef.current = undefined;
      unlisten?.();
      if (registered) void invoke("desktop_unregister_source_draft_lifecycle_renderer", { token: registrationToken });
    };
  }, [complete, completionBlocked, onProtectionChange, setCurrentRequest, sourceDraftDirtyRef]);

  if (!request) return null;
  const isExit = request.kind === "exit";
  const journalBusy = journalActionBusyRef.current;
  const sourceBusy = sourceDraftActionBusyRef.current;
  const canDiscard = !busy && !completionBlocked() && requestError === null;
  const heading = sourceDraftDirtyRef.current
    ? isExit ? "Discard unsaved proposals and quit Bridge?" : "Discard unsaved proposals and close this window?"
    : isExit ? "Quit Bridge?" : "Close this window?";
  const description = journalBusy
    ? "A Journal action is in progress. Wait for the result before closing Bridge."
    : sourceBusy
    ? "A local source-draft action is in progress. Wait for its result before closing Bridge."
    : sourceDraftDirtyRef.current
    ? "Unsaved proposal edits are local only and will be lost."
    : "No unsaved proposals remain.";

  return createPortal(
    <div className="source-draft-lifecycle-backdrop">
      <section
        className="source-draft-confirm source-draft-lifecycle-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="source-draft-lifecycle-heading"
        tabIndex={-1}
        ref={dialogRef}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !busy) {
            void cancel();
            return;
          }
          trapDrawerTabKeydown(event);
        }}
      >
        <div>
          <h3 id="source-draft-lifecycle-heading">{heading}</h3>
          <p>{description}</p>
          {requestError && <p className="error-banner" role="alert">{requestError}</p>}
        </div>
        <div className="source-draft-actions">
          <button className="primary" type="button" onClick={() => void complete()} disabled={!canDiscard}>
            {isExit ? "Discard and quit" : "Discard and close"}
          </button>
          <button className="secondary-action" type="button" onClick={() => void cancel()} disabled={busy}>
            Keep editing
          </button>
        </div>
      </section>
    </div>,
    document.body,
  );
}
