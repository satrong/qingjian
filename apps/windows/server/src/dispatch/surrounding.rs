//! 应用前后文：同步本地重排与云联想，处理编辑会话晚于按键的回报。

use qingjian_core::SurroundingText;
use qingjian_platform::protocol::SessionId;

use super::Router;
use super::composed::Composed;

impl Router {
    /// 前文交给本地重排，前后文交给云联想；已结束、非聚焦或私密会话的回报丢弃。
    pub(super) fn set_surrounding(&mut self, session: SessionId, text: String, after: String) {
        if self.focused != Some(session)
            || self.engine.composition().is_empty()
            || self.engine.is_private()
        {
            return;
        }
        self.surrounding = Some(SurroundingText {
            before: text.clone(),
            after,
        });
        self.engine
            .set_rescoring_context((!text.is_empty()).then_some(text));
        if matches!(self.composed, Some(Composed::Candidates { .. })) {
            // 查一次只为按新前文重新记下要打分的文本，候选顺序此刻不变
            let _ = self.engine.query();
            self.schedule_rescoring();
        }
        // 编辑会话可能晚于最后一次按键；不等下一键，立即替换尚在途中的无上下文请求。
        if let Some(Composed::Candidates { layout, .. }) = self.composed.as_mut() {
            layout.set_cloud(Vec::new());
            self.sentence = None;
            self.engine
                .request_prediction(self.surrounding.clone(), layout.local());
        }
    }
}
