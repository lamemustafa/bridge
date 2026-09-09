use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceDraftLifecycleKind {
    Close,
    Exit,
}

/// The renderer must echo this opaque ID with the kind. That prevents a
/// delayed response for one close from authorizing a later close.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct SourceDraftLifecycleRequest {
    pub(crate) request_id: Uuid,
    pub(crate) kind: SourceDraftLifecycleKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum RendererLifecycleProtection {
    #[default]
    NeverReady,
    Ready,
    LostAfterReady,
}

#[derive(Default)]
pub(crate) struct SourceDraftLifecycleGuard {
    pending: Mutex<Option<SourceDraftLifecycleRequest>>,
    renderer_protection: Mutex<(RendererLifecycleProtection, Option<Uuid>)>,
    close_permitted: AtomicBool,
    exit_permitted: AtomicBool,
    main_close_permitted: AtomicBool,
    exit_after_main_close_permitted: AtomicBool,
}

impl SourceDraftLifecycleGuard {
    /// Before the renderer completes its listener handshake, it cannot create
    /// source-draft or Journal work. Native close stays available in that
    /// narrow startup state. A renderer that was ready and later disappeared
    /// remains guarded because local unsaved work may already exist.
    pub(crate) fn requires_confirmation(&self) -> bool {
        self.lock_renderer_protection().0 != RendererLifecycleProtection::NeverReady
    }

    pub(crate) fn renderer_registered(&self, token: Uuid) {
        *self.lock_renderer_protection() = (RendererLifecycleProtection::Ready, Some(token));
    }

    pub(crate) fn renderer_unregistered(&self, token: Uuid) {
        let mut protection = self.lock_renderer_protection();
        if protection.0 == RendererLifecycleProtection::Ready && protection.1 == Some(token) {
            *protection = (RendererLifecycleProtection::LostAfterReady, None);
        }
    }

    pub(crate) fn request(&self, kind: SourceDraftLifecycleKind) -> SourceDraftLifecycleRequest {
        let mut pending = self.lock_pending();
        pending
            .get_or_insert_with(|| SourceDraftLifecycleRequest {
                request_id: Uuid::new_v4(),
                kind,
            })
            .clone()
    }

    pub(crate) fn pending(&self) -> Option<SourceDraftLifecycleRequest> {
        self.lock_pending().clone()
    }

    pub(crate) fn cancel(&self, request: &SourceDraftLifecycleRequest) -> bool {
        let mut pending = self.lock_pending();
        if pending.as_ref() != Some(request) {
            return false;
        }
        *pending = None;
        true
    }

    pub(crate) fn authorize(&self, request: &SourceDraftLifecycleRequest) -> bool {
        let mut pending = self.lock_pending();
        if pending.as_ref() != Some(request) {
            return false;
        }
        *pending = None;
        match request.kind {
            SourceDraftLifecycleKind::Close => {
                self.close_permitted.store(true, Ordering::Release);
            }
            SourceDraftLifecycleKind::Exit => {
                self.exit_permitted.store(true, Ordering::Release);
            }
        }
        true
    }

    pub(crate) fn restore_after_failed_close(&self, request: SourceDraftLifecycleRequest) {
        self.close_permitted.store(false, Ordering::Release);
        let mut pending = self.lock_pending();
        if pending.is_none() {
            *pending = Some(request);
        }
    }

    pub(crate) fn take_close_permit(&self) -> bool {
        let permitted = self.close_permitted.swap(false, Ordering::AcqRel);
        if permitted {
            self.main_close_permitted.store(true, Ordering::Release);
        }
        permitted
    }

    pub(crate) fn take_exit_permit(&self) -> bool {
        let explicit = self.exit_permitted.swap(false, Ordering::AcqRel);
        let chained_close = self
            .exit_after_main_close_permitted
            .swap(false, Ordering::AcqRel);
        explicit || chained_close
    }

    pub(crate) fn main_window_destroyed(&self) {
        // Once the only renderer is gone, preventing the following exit would
        // strand a headless process with no place to show a confirmation.
        self.main_close_permitted.store(false, Ordering::Release);
        self.exit_after_main_close_permitted
            .store(true, Ordering::Release);
    }

    #[cfg(test)]
    fn renderer_is_ready(&self) -> bool {
        self.lock_renderer_protection().0 == RendererLifecycleProtection::Ready
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, Option<SourceDraftLifecycleRequest>> {
        self.pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_renderer_protection(
        &self,
    ) -> std::sync::MutexGuard<'_, (RendererLifecycleProtection, Option<Uuid>)> {
        self.renderer_protection
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_same_kind_completion_cannot_authorize_a_later_request() {
        let guard = SourceDraftLifecycleGuard::default();
        guard.renderer_registered(Uuid::new_v4());
        let first = guard.request(SourceDraftLifecycleKind::Close);
        assert!(guard.cancel(&first));

        let second = guard.request(SourceDraftLifecycleKind::Close);
        assert_ne!(first.request_id, second.request_id);
        assert!(!guard.authorize(&first));
        assert!(guard.authorize(&second));
        assert!(guard.take_close_permit());
        assert!(!guard.take_close_permit());
    }

    #[test]
    fn exit_consumes_every_permit_even_if_both_are_set() {
        let guard = SourceDraftLifecycleGuard::default();
        guard.renderer_registered(Uuid::new_v4());
        let exit = guard.request(SourceDraftLifecycleKind::Exit);
        assert!(guard.authorize(&exit));
        let close = guard.request(SourceDraftLifecycleKind::Close);
        assert!(guard.authorize(&close));
        assert!(guard.take_close_permit());
        guard.main_window_destroyed();

        assert!(guard.take_exit_permit());
        assert!(!guard.take_exit_permit());
    }

    #[test]
    fn close_allows_a_chained_exit_only_after_main_window_destruction() {
        let guard = SourceDraftLifecycleGuard::default();
        guard.renderer_registered(Uuid::new_v4());
        let close = guard.request(SourceDraftLifecycleKind::Close);
        assert!(guard.authorize(&close));
        assert!(guard.take_close_permit());
        assert!(!guard.take_exit_permit());

        guard.main_window_destroyed();
        assert!(guard.take_exit_permit());
        assert!(!guard.take_exit_permit());

        let unapproved = SourceDraftLifecycleGuard::default();
        unapproved.main_window_destroyed();
        assert!(unapproved.take_exit_permit());
    }
    #[test]
    fn startup_is_closable_but_a_lost_renderer_stays_protected() {
        let guard = SourceDraftLifecycleGuard::default();
        assert!(!guard.requires_confirmation());

        let renderer_token = Uuid::new_v4();
        guard.renderer_registered(renderer_token);
        assert!(guard.requires_confirmation());

        guard.renderer_unregistered(renderer_token);
        assert!(guard.requires_confirmation());

        let replacement_token = Uuid::new_v4();
        guard.renderer_registered(replacement_token);
        guard.renderer_unregistered(renderer_token);
        assert!(guard.renderer_is_ready());
    }
}
