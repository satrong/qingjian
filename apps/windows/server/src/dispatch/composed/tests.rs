//! 云联想失败时的组句帧展示。

use qingjian_core::{Engine, Prediction, PredictionPolicy, PredictionRequest, Predictor};
use qingjian_dictionary::Dictionary;

use super::Router;
use crate::dispatch::RouterConfig;

struct FailingPredictor(Option<Prediction>);

impl Predictor for FailingPredictor {
    fn policy(&self) -> PredictionPolicy {
        PredictionPolicy::default()
    }

    fn submit(&mut self, _request: PredictionRequest) {}

    fn poll(&mut self) -> Option<Prediction> {
        self.0.take()
    }
}

#[test]
fn cloud_failure_appears_in_candidate_frame_until_next_input() {
    let dictionary = Dictionary::parse("开发\tkai fa\t100\n").unwrap();
    let engine =
        Engine::new(dictionary).with_predictor(Box::new(FailingPredictor(Some(Prediction {
            sequence: 1,
            failed: true,
            ..Prediction::default()
        }))));
    let mut router = Router::new(engine, RouterConfig::default());
    router.engine.set_input("kaifa");
    router.recompose();
    router.poll_prediction();
    let frame = router.current_frame();
    assert_eq!(
        frame.notice.as_deref(),
        Some("云联想失败，请在云服务中测试连接")
    );
    assert_eq!(frame.candidates.items[0].text, "开发");

    router.engine.set_input("kaifang");
    router.recompose();
    assert!(router.current_frame().notice.is_none());
}
