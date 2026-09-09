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

#[derive(Default)]
pub(crate) struct SourceDraftLifecycleGuard {
    pending: Mutex<Option<SourceDraftLifecycleRequest>>,
    close_permitted: AtomicBool,
    exit_permitted: AtomicBool,
    main_close_permitted: AtomicBool,
    exit_after_main_close_permitted: AtomicBool,
}

impl SourceDraftLifecycleGuard {
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

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, Option<SourceDraftLifecycleRequest>> {
        self.pending
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
}
