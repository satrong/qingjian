//! 往文档写字：按键结果经异步编辑会话写上屏文本 + 组句拼音行；失焦 / 停用 / 切模式时让 Server 交出缓冲区原样落定；
//! 翻译选中文字时起只读会话读选区。

use windows::Win32::UI::TextServices::ITfContext;
use windows::core::Ref;

use super::TextService_Impl;
use crate::com::edit::{request_selection, request_update};
use crate::com::log::log;

impl TextService_Impl {
    pub(super) fn read_selection(&self, pic: Ref<ITfContext>, request: u64) {
        let Ok(context) = pic.ok() else {
            log("翻译选中文字：无上下文，取消读选区");
            return;
        };
        if let Err(error) = request_selection(
            context,
            self.client_id.get(),
            self.engine.clone(),
            self.shared.clone(),
            request,
        ) {
            log(&format!("请求读选区会话失败: {error}"));
        }
    }

    /// 失焦 / 停用 / 切模式：让 Server 交出缓冲区，原样落进最近收键的文档并收掉组句。
    /// 组句已被应用终止的（拼音已是普通文本）只清 Server 不再插。
    pub(super) fn commit_pending(&self) {
        if self.shared.translating() {
            self.shared.set_translating(false);
            self.shared.hide_candidates();
        }
        let stale = self.shared.take_server_stale();
        if !self.shared.composing() && !stale {
            return;
        }
        let text = {
            let mut guard = self.engine.borrow_mut();
            let Some(client) = guard.as_mut() else {
                self.shared.reset();
                return;
            };
            match client.commit() {
                Ok(text) => text,
                Err(error) => {
                    log(&format!("失焦上屏失败，断开，下一键重连: {error}"));
                    drop(guard);
                    self.disconnect();
                    return;
                }
            }
        };
        if stale {
            return;
        }
        self.shared.end_composing();
        let Some(context) = self.shared.last_context() else {
            log(&format!("失焦上屏没有上下文，丢弃: {text:?}"));
            self.shared.reset();
            return;
        };
        log(&format!("失焦上屏: {text:?}"));
        let requested = request_update(
            &context,
            self.client_id.get(),
            self.engine.clone(),
            self.shared.clone(),
            text.filter(|t| !t.is_empty()),
            String::new(),
        );
        if let Err(error) = requested {
            log(&format!("失焦上屏的编辑会话没被受理: {error}"));
            self.shared.reset();
        }
    }

    /// 手机推送来的一段文本：插进当前输入框（`insertText:` 那一路，不经组句）。
    ///
    /// 落点与失焦上屏一样是「最近拿到的上下文 + 一次异步编辑会话」；Server 侧已经把组句清了，
    /// 所以这里只有上屏文本、没有拼音行。插入期间这个进程是前台才收得到（`Poll` / `SyncMode`
    /// 都是前台节拍），所以不用再问一次焦点。
    pub(super) fn insert_remote_text(&self, text: String) {
        log(&format!("手机推送上屏: {text}"));
        let Some(context) = self.edit_context() else {
            log("手机推送没有可插入的上下文，丢弃");
            return;
        };
        if let Err(error) = request_update(
            &context,
            self.client_id.get(),
            self.engine.clone(),
            self.shared.clone(),
            Some(text),
            String::new(),
        ) {
            log(&format!("手机推送的编辑会话没被受理: {error}"));
        }
    }

    /// 该往哪儿插：优先最近拿到的编辑上下文（按键路径给的，最准）；
    /// 没有就问线程管理器要当前焦点所在文档的顶层上下文——用户刚点进输入框、还没敲过键时只有这个
    /// （`Activate` 只给线程管理器，不给上下文，所以那时 `last_context` 还是空的）。
    ///
    /// 顶层上下文是文档的主编辑视图：光标在文档里别的编辑框（对话框里的输入框之类）时可能插错地方，
    /// 那种情况等用户敲一下键，`last_context` 就换成准的了。
    fn edit_context(&self) -> Option<ITfContext> {
        if let Some(context) = self.shared.last_context() {
            return Some(context);
        }
        let thread_mgr = self.thread_mgr.borrow().clone()?;
        // SAFETY: 两个都是纯查询：取焦点所在文档，再取它的顶层上下文
        unsafe { thread_mgr.GetFocus().ok()?.GetTop().ok() }
    }

    /// 经异步编辑会话把上屏文本 + 组句拼音行写进文档。
    pub(super) fn update_document(
        &self,
        pic: Ref<ITfContext>,
        commit: Option<String>,
        preedit: String,
    ) {
        // 退到最后一个字母时帧已空但组句句柄还在，得跑一次把它收掉。
        if commit.is_none()
            && preedit.is_empty()
            && !self.shared.composing()
            && !self.shared.has_composition()
        {
            return;
        }
        let Ok(context) = pic.ok() else {
            log(&format!(
                "无上下文，丢弃更新: commit={commit:?} preedit={preedit:?}"
            ));
            return;
        };
        if let Err(error) = request_update(
            context,
            self.client_id.get(),
            self.engine.clone(),
            self.shared.clone(),
            commit,
            preedit,
        ) {
            log(&format!("请求组句更新失败: {error}"));
        }
    }
}
