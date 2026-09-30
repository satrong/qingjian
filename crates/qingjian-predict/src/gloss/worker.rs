use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use qingjian_core::{FilledGloss, Language};

use super::prompt;
use crate::chat_client::ChatClient;
use crate::error::PredictError;

/// 收到第一个词后最多等这么久再发，攒同一批。
pub const BATCH_WAIT: Duration = Duration::from_millis(1500);

/// 一批最多几个词。
pub const BATCH_SIZE: usize = 8;

/// 回复的 token 上限：8 个词各两条短译词绰绰有余。
const MAX_TOKENS: u32 = 600;

/// 后台线程：攒词、发请求、回释义。
pub struct GlossWorker {
    /// 请求入口。主线程 drop 掉发送端后线程自然退出。
    requests: UnboundedReceiver<(Language, String)>,

    /// 结果出口。
    responses: Sender<FilledGloss>,

    /// 网络客户端。
    client: ChatClient,

    /// 本进程内问过的 (语言, 词)，不再问。
    asked: HashSet<(Language, String)>,
}

impl GlossWorker {
    pub fn new(
        requests: UnboundedReceiver<(Language, String)>,
        responses: Sender<FilledGloss>,
        client: ChatClient,
    ) -> Self {
        Self {
            requests,
            responses,
            client,
            asked: HashSet::new(),
        }
    }

    /// 阻塞运行直到发送端全部关闭。
    pub fn run(self) -> Result<(), PredictError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        // 等待词条、攒批时也驱动 HTTP 连接任务，及时移除被对端关闭的空闲连接。
        runtime.block_on(self.run_async());
        Ok(())
    }

    async fn run_async(mut self) {
        while let Some(first) = self.requests.recv().await {
            let Some(batch) = self.collect(first).await else {
                return;
            };
            // 一批里可能混着两种语言（中途切了学习语言）：按语言各发一次
            let mut languages: Vec<Language> = Vec::new();
            for (language, _) in &batch {
                if !languages.contains(language) {
                    languages.push(*language);
                }
            }
            for language in languages {
                let words: Vec<String> = batch
                    .iter()
                    .filter(|(l, _)| *l == language)
                    .map(|(_, w)| w.clone())
                    .collect();
                self.ask(language, words).await;
            }
        }
    }

    /// 从第一个词起攒一批：等到 [`BATCH_WAIT`] 或攒够 [`BATCH_SIZE`]；问过的跳过。发送端关闭返回 `None`。
    async fn collect(&mut self, first: (Language, String)) -> Option<Vec<(Language, String)>> {
        let deadline = tokio::time::Instant::now() + BATCH_WAIT;
        let mut batch = Vec::with_capacity(BATCH_SIZE);
        self.take(&mut batch, first);
        while batch.len() < BATCH_SIZE {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            match tokio::time::timeout_at(deadline, self.requests.recv()).await {
                Ok(Some(item)) => self.take(&mut batch, item),
                Err(_) => break,
                Ok(None) => return None,
            }
        }
        Some(batch)
    }

    fn take(&mut self, batch: &mut Vec<(Language, String)>, item: (Language, String)) {
        if self.asked.insert(item.clone()) {
            batch.push(item);
        }
    }

    async fn ask(&self, language: Language, words: Vec<String>) {
        if words.is_empty() {
            return;
        }
        let start = Instant::now();
        let system = prompt::system_prompt(language);
        let user = prompt::user_prompt(&words);
        match self.client.chat(system, &user, MAX_TOKENS).await {
            Ok(content) => {
                let filled = prompt::parse_reply(&content, language, &words);
                tracing::info!(
                    language = language.code(),
                    asked = words.len(),
                    filled = filled.len(),
                    elapsed_ms = start.elapsed().as_millis(),
                    "释义兜底完成"
                );
                for gloss in filled {
                    // 接收端没了说明 Filler 已经被 drop，线程随后也会退出
                    if self.responses.send(gloss).is_err() {
                        return;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(language = language.code(), asked = words.len(), %error, "释义兜底失败");
            }
        }
    }
}
