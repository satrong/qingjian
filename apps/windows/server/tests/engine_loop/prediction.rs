//! 应用上下文经协议送到云联想，覆盖编辑会话晚于按键回包的时序。

use std::sync::{Arc, Mutex};

use qingjian_core::{Prediction, PredictionPolicy, PredictionRequest, Predictor};
use qingjian_platform::protocol::{ClientMessage, SessionId};
use qingjian_windows_server::Router;

use crate::support::{
    SESSION, digit, function_key, letter, open_session, press, press_in, router, slot_of,
    type_letters,
};

#[derive(Clone, Default)]
struct RecordingPredictor(Arc<Mutex<Vec<PredictionRequest>>>);

impl Predictor for RecordingPredictor {
    fn policy(&self) -> PredictionPolicy {
        PredictionPolicy::default()
    }

    fn submit(&mut self, request: PredictionRequest) {
        self.0.lock().unwrap().push(request);
    }

    fn poll(&mut self) -> Option<Prediction> {
        None
    }
}

#[test]
fn prediction_receives_surrounding_after_keys() {
    let mut router = router();
    let predictor = RecordingPredictor::default();
    router
        .engine_mut()
        .set_predictor(Box::new(predictor.clone()));
    type_letters(&mut router, "nihao");
    router.handle(ClientMessage::Surrounding {
        session: SESSION,
        text: "今天".into(),
        after: "，一起去吧".into(),
    });
    let requests = predictor.0.lock().unwrap();
    let request = requests.last().expect("应发起联想");
    assert_eq!(request.before, "今天");
    assert_eq!(request.after, "，一起去吧");
}

fn setup() -> (Router, RecordingPredictor) {
    let mut router = router();
    let predictor = RecordingPredictor::default();
    router
        .engine_mut()
        .set_predictor(Box::new(predictor.clone()));
    type_letters(&mut router, "nihao");
    surrounding(&mut router, SESSION, "今天", "一起去");
    (router, predictor)
}

fn surrounding(router: &mut Router, session: SessionId, before: &str, after: &str) {
    router.handle(ClientMessage::Surrounding {
        session,
        text: before.into(),
        after: after.into(),
    });
}

fn context(predictor: &RecordingPredictor) -> (String, String) {
    let requests = predictor.0.lock().unwrap();
    let request = requests.last().unwrap();
    (request.before.clone(), request.after.clone())
}

#[test]
fn prediction_keeps_context_on_next_key_and_clears_unavailable_sides() {
    let (mut router, predictor) = setup();
    press(&mut router, letter('m'));
    assert_eq!(context(&predictor), ("今天".into(), "一起去".into()));
    surrounding(&mut router, SESSION, "", "文档开头");
    assert_eq!(context(&predictor), ("".into(), "文档开头".into()));
    surrounding(&mut router, SESSION, "", "");
    assert_eq!(context(&predictor), ("".into(), "".into()));
}

#[test]
fn prediction_drops_context_when_composition_ends() {
    let (mut router, predictor) = setup();
    press(&mut router, function_key(0x1b));
    let count = predictor.0.lock().unwrap().len();
    surrounding(&mut router, SESSION, "迟到的上下文", "迟到的后文");
    assert_eq!(predictor.0.lock().unwrap().len(), count);
    type_letters(&mut router, "nihao");
    assert_eq!(context(&predictor), ("".into(), "".into()));
}

#[test]
fn prediction_context_belongs_to_focused_session() {
    let (mut router, predictor) = setup();
    surrounding(&mut router, SessionId(2), "别的应用", "别的后文");
    assert_eq!(context(&predictor), ("今天".into(), "一起去".into()));
    open_session(&mut router, SessionId(2), None);
    for c in "nihao".chars() {
        press_in(&mut router, SessionId(2), letter(c));
    }
    assert_eq!(context(&predictor), ("".into(), "".into()));
}

#[test]
fn prediction_does_not_accept_private_context() {
    let (mut router, predictor) = setup();
    router.handle(ClientMessage::Privacy {
        session: SESSION,
        private: true,
    });
    let count = predictor.0.lock().unwrap().len();
    surrounding(&mut router, SESSION, "私密文字", "私密后文");
    press(&mut router, letter('m'));
    assert_eq!(predictor.0.lock().unwrap().len(), count);
    router.handle(ClientMessage::Privacy {
        session: SESSION,
        private: false,
    });
    press(&mut router, letter('a'));
    assert_eq!(context(&predictor), ("".into(), "".into()));
}

#[test]
fn prediction_refreshes_context_after_partial_commit() {
    let (mut router, predictor) = setup();
    let (_, _, frame) = press(&mut router, letter('m'));
    let slot = slot_of(&frame, "你");
    let (_, commit, frame) = press(&mut router, digit(slot));
    assert_eq!(commit.as_deref(), Some("你"));
    assert!(!frame.is_empty());
    assert_eq!(context(&predictor), ("".into(), "".into()));
    surrounding(&mut router, SESSION, "今天你", "一起去");
    assert_eq!(context(&predictor), ("今天你".into(), "一起去".into()));
}

#[test]
fn prediction_clips_application_context_by_unicode_characters() {
    let (mut router, predictor) = setup();
    surrounding(&mut router, SESSION, &"𠮷".repeat(80), &"😀".repeat(50));
    assert_eq!(context(&predictor), ("𠮷".repeat(64), "😀".repeat(32)));
}
