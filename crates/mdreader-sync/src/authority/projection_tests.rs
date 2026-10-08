use super::*;

fn active(controller: Arc<SessionController>) -> ProjectionPermit {
    let scope = SyncScope::new(
        "a".repeat(64),
        "11111111-1111-4111-8111-111111111111".into(),
        "A".repeat(43),
    )
    .unwrap();
    let lease = controller
        .activate(controller.epoch().unwrap(), scope)
        .unwrap();
    ProjectionPermit {
        controller,
        lease,
        expires: now().unwrap() + 60,
    }
}

#[test]
fn same_scope_reactivation_and_controller_restart_cannot_reuse_old_projection() {
    let controller = SessionController::new();
    let old = active(controller.clone());
    controller.logout().unwrap();
    let fresh = active(controller);
    assert_eq!(old.scope(), fresh.scope());
    assert_ne!(old.session_token(), fresh.session_token());
    let mut called = false;
    assert!(old.with_current(|_| called = true).is_err());
    assert!(!called);
    fresh.with_current(|guard| guard.check()).unwrap().unwrap();
    let restarted = active(SessionController::new());
    assert_eq!(restarted.scope(), fresh.scope());
    assert_ne!(restarted.session_token(), fresh.session_token());
}

#[test]
fn publication_holds_session_barrier_and_expired_permits_never_call_owner() {
    let controller = SessionController::new();
    let mut permit = active(controller.clone());
    permit
        .with_current(|guard| {
            assert!(controller.binding.try_write().is_err());
            guard.check().unwrap();
        })
        .unwrap();
    assert!(controller.binding.try_write().is_ok());
    permit.expires = now().unwrap();
    let mut called = false;
    assert!(permit.with_current(|_| called = true).is_err());
    assert!(!called);
}
