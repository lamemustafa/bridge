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
