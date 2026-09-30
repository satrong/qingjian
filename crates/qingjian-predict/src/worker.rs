//! 后台线程：收请求、防抖、查缓存、发网络请求、回结果。

use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use qingjian_core::{Prediction, PredictionRequest};

use crate::prompt::Reply;

use crate::cache::PredictionCache;
use crate::chat_client::ChatClient;
use crate::error::PredictError;

/// 缓存条数。
const CACHE_CAPACITY: usize = 64;

pub struct Worker {
    /// 请求入口。主线程 drop 掉发送端后线程自然退出。
    requests: UnboundedReceiver<PredictionRequest>,

    /// 结果出口。
    responses: Sender<Prediction>,

    /// 网络客户端。
    client: ChatClient,

    /// 防抖窗口。
    debounce: Duration,

    /// 结果缓存。
    cache: PredictionCache,
}

/// 缓存里存的是解析后的回复。
type Cached = Reply;

impl Worker {
    pub fn new(
        requests: UnboundedReceiver<PredictionRequest>,
        responses: Sender<Prediction>,
        client: ChatClient,
        debounce: Duration,
    ) -> Self {
        Self {
            requests,
            responses,
            client,
            debounce,
            cache: PredictionCache::with_capacity(CACHE_CAPACITY),
        }
    }

    /// 阻塞运行直到发送端全部关闭。
    pub fn run(self) -> Result<(), PredictError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        // 整个收件循环都要驱动 runtime，否则空闲期间 HTTP 连接任务无法处理对端断连。
        runtime.block_on(self.run_async());
        Ok(())
    }

    async fn run_async(mut self) {
        while let Some(first) = self.requests.recv().await {
            let Some(request) = self.debounce(first).await else {
                return;
            };
            let key = PredictionCache::key(&request);
            if let Some(reply) = self.cache.get(&key) {
                tracing::debug!(sequence = request.sequence, kind = ?request.kind, "联想命中缓存");
                self.reply(request.sequence, reply.clone());
                continue;
            }
            let start = Instant::now();
            match self.client.complete(&request).await {
                Ok(reply) => {
                    tracing::info!(
                        sequence = request.sequence,
                        elapsed_ms = start.elapsed().as_millis(),
                        words = reply.words.len(),
                        sentence = reply.sentence.is_some(),
                        "联想完成"
                    );
                    // 空回复不进缓存：模型偶尔什么都不给（问字尤其），缓存住就等于这个问题以后永远没答案，
                    // 用户重打一遍也只会命中缓存（2026-09-07 `?mumumu` 就是这么卡住的）
                    if !reply.is_empty() {
                        self.cache.insert(key, reply.clone());
                    }
                    self.reply(request.sequence, reply);
                }
                Err(error) => {
                    tracing::warn!(sequence = request.sequence, %error, "联想失败");
                    let _ = self.responses.send(Prediction {
                        sequence: request.sequence,
                        failed: true,
                        ..Prediction::default()
                    });
                }
            }
        }
    }

    /// 防抖：在窗口内持续收到新请求就一直等，只保留最后一个。发送端关闭返回 `None`。
    async fn debounce(&mut self, mut latest: PredictionRequest) -> Option<PredictionRequest> {
        loop {
            match tokio::time::timeout(self.debounce, self.requests.recv()).await {
                Ok(Some(newer)) => latest = newer,
                Err(_) => return Some(latest),
                Ok(None) => return None,
            }
        }
    }

    fn reply(&self, sequence: u64, reply: Cached) {
        // 接收端没了说明 Predictor 已经被 drop，线程随后也会退出
        let _ = self.responses.send(Prediction {
            sequence,
            words: reply.words,
            sentence: reply.sentence,
            failed: false,
        });
    }
}
