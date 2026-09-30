//! 无障碍回包与组句生命周期：过期、失焦、私密及旧组句都不能复用结果。

use std::sync::mpsc;
use std::time::{Duration, Instant};

use qingjian_core::{Engine, SurroundingText};
use qingjian_dictionary::Dictionary;
use qingjian_platform::protocol::{ClientMessage, SessionId};

use super::Pending;
use crate::dispatch::{Router, RouterConfig};

fn pending() -> (Pending, mpsc::SyncSender<Option<SurroundingText>>) {
    let (send, receive) = mpsc::sync_channel(1);
    (
        Pending {
            session: SessionId(1),
            window: 0,
            started: Instant::now(),
            receive,
        },
        send,
    )
}

fn context() -> SurroundingText {
    SurroundingText {
        before: "今天".into(),
        after: "一起去公园".into(),
    }
}

fn router() -> Router {
    let dictionary = Dictionary::parse("开发\tkai fa\t100\n").unwrap();
    let mut router = Router::new(Engine::new(dictionary), RouterConfig::default());
    router.ensure_focus(SessionId(1));
    router.engine.set_input("kaifa");
    router.recompose();
    router
}

#[test]
fn accepts_ready_context_only_for_same_session_window_and_deadline() {
    let (pending, send) = pending();
    assert_eq!(
        pending.poll_at(Some(SessionId(1)), true, pending.started),
        None
    );
    send.send(Some(context())).unwrap();
    // 拒绝检查不能误消费旧回包。
    assert_eq!(
        pending.poll_at(Some(SessionId(2)), true, pending.started),
        Some(None)
    );
    assert_eq!(
        pending.poll_at(Some(SessionId(1)), false, pending.started),
        Some(None)
    );
    assert_eq!(
        pending.poll_at(
            Some(SessionId(1)),
            true,
            pending.started + Duration::from_millis(501)
        ),
        Some(None)
    );
    assert_eq!(
        pending.poll_at(Some(SessionId(1)), true, pending.started),
        Some(Some(context()))
    );
}

#[test]
fn commit_then_new_composition_cannot_receive_previous_context() {
    let mut router = router();
    let (pending, send) = pending();
    router.accessibility = Some(pending);
    router.commit_raw_for(SessionId(1));
    router.engine.set_input("kaifa");
    router.recompose();
    assert!(router.accessibility.is_none());
    assert!(send.send(Some(context())).is_err());
}

#[test]
fn privacy_focus_and_empty_composition_cancel_pending_read() {
    for action in 0..3 {
        let mut router = router();
        let (pending, send) = pending();
        router.accessibility = Some(pending);
        match action {
            0 => router.set_privacy(SessionId(1), true),
            1 => router.ensure_focus(SessionId(2)),
            _ => {
                router.engine.clear();
                router.recompose();
            }
        }
        assert!(router.accessibility.is_none());
        assert!(send.send(Some(context())).is_err());
    }
}

#[test]
fn tsf_result_supersedes_pending_accessibility_result() {
    let mut router = router();
    let (pending, send) = pending();
    router.accessibility = Some(pending);
    router.handle(ClientMessage::Surrounding {
        session: SessionId(1),
        text: "新的前文".into(),
        after: "新的后文".into(),
    });
    assert!(router.accessibility.is_none());
    assert!(send.send(Some(context())).is_err());
    assert_eq!(router.surrounding.as_ref().unwrap().before, "新的前文");
}
