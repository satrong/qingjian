# 云联想「额外参数」（extra_body）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 云服务页新增「额外参数」JSON 输入框，内容原样合并进发往 LLM 的请求体，同名键覆盖内置值。

**Architecture:** 配置存 JSON 文本 `PredictConfig.extra_body`（留空不发）；`ChatClient::new` 解析一次成 `Option<serde_json::Map>`，`ChatClient::chat` 构建完请求体（含 `ThinkingSwitch` 改写）后合并——联想、问字、翻译、测试连接共用这条路径，全部自动生效。macOS 偏好设置加多行文本框，保存时校验，请求侧解析兜底。

**Tech Stack:** Rust workspace（qingjian-predict / qingjian-platform / apps/macos）、serde_json、objc2 AppKit。

**设计文档:** `docs/design/extra-body.md`

## Global Constraints

- 注释与 UI 文案用中文，代码标识符英文；新字段都要 `///`；提交信息 Conventional Commits（类型与范围英文小写、说明中文，如 `feat(predict): …`），不加 AI 署名。
- 钩子已启用：pre-commit 跑 fmt + clippy `-D warnings`，commit-msg 查格式；不要 `--no-verify`。
- 导入写精确路径，不用 glob（`#[cfg(test)] mod tests` 里 `use super::*` 除外）。
- 单文件不超 800 行；本次不新建源码文件，只改既有文件。
- 文档同步：配置项进模板与 `docs/notes/crate-notes.md`；用户可感知的改动同一个提交改 `docs/user/` 对应页。
- 验证命令：`cargo test -p <crate>` 单测、`cargo clippy --all-targets -- -D warnings`。

---

### Task 1: qingjian-predict —— `extra_body` 字段与请求体合并

**Files:**
- Modify: `crates/qingjian-predict/src/config.rs`（`PredictConfig` 字段与默认值）
- Modify: `crates/qingjian-predict/src/chat_client.rs`（解析、合并、单测）

**Interfaces:**
- Consumes: `PredictConfig`（已有 serde `#[serde(default)]`，`ChatClient::new(config: &PredictConfig, api_key: String)`）。
- Produces: `PredictConfig.extra_body: String`（TOML 键 `[predict] extra_body`）；`parse_extra_body(&str) -> Option<serde_json::Map<String, serde_json::Value>>` 与 `merge_extra_body(&mut serde_json::Value, &Map)`（chat_client.rs 私有自由函数，供单测）。

- [ ] **Step 1: 写失败的单测（解析 + 合并）**

在 `crates/qingjian-predict/src/chat_client.rs` 的 `mod tests` 里（`use super::*;` 之后）加：

```rust
#[test]
fn extra_body_parses_objects_and_ignores_the_rest() {
    assert!(parse_extra_body("").is_none());
    assert!(parse_extra_body("   ").is_none());
    assert_eq!(
        parse_extra_body(" {\"a\": 1} ").unwrap()["a"],
        serde_json::json!(1)
    );
    assert!(parse_extra_body("[1, 2]").is_none());
    assert!(parse_extra_body("not json").is_none());
}

#[test]
fn extra_body_overrides_built_in_fields() {
    let mut body = serde_json::json!({ "model": "glm", "temperature": 0.3 });
    let extra =
        parse_extra_body(r#"{"temperature": 0.9, "thinking": {"type": "disabled"}}"#).unwrap();
    merge_extra_body(&mut body, &extra);
    assert_eq!(body["temperature"], 0.9);
    assert_eq!(body["thinking"]["type"], "disabled");
    assert_eq!(body["model"], "glm");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p qingjian-predict extra_body`
Expected: 编译失败，`parse_extra_body` / `merge_extra_body` 未定义。

- [ ] **Step 3: 最小实现**

`crates/qingjian-predict/src/config.rs`：`system_prompt` 字段后加：

```rust
    /// 额外请求参数：JSON 对象文本，原样合并进发往接口的请求体，同名键覆盖内置值。
    /// 留空不发；不是合法 JSON 对象时构建客户端记一条警告后整个忽略。
    pub extra_body: String,
```

`Default` 里 `system_prompt: String::new(),` 后加：

```rust
            extra_body: String::new(),
```

`crates/qingjian-predict/src/chat_client.rs`：

`ChatClient` 结构体 `system_prompt` 字段后加：

```rust
    /// 用户补充的请求体字段；构建客户端时解析好，请求时直接合并，同名键覆盖内置值。
    extra_body: Option<serde_json::Map<String, serde_json::Value>>,
```

`ChatClient::new` 的 `Self { … }` 里 `system_prompt: config.system_prompt.clone(),` 后加：

```rust
            extra_body: parse_extra_body(&config.extra_body),
```

`chat` 里这段之后：

```rust
        if matches!(self.reasoning_effort, Some(ReasoningEffort::None)) {
            self.thinking_switch.disable(&mut body);
        }
```

加（合并必须是最后一步，用户 JSON 覆盖包括 `thinking` 在内的一切内置值）：

```rust
        if let Some(extra) = &self.extra_body {
            merge_extra_body(&mut body, extra);
        }
```

`ThinkingSwitch` 的 `impl` 之后加两个自由函数：

```rust
/// 配置里的额外参数解析成 JSON 对象；留空不发，不是对象或不是 JSON 记一条警告后不用。
fn parse_extra_body(value: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(serde_json::Value::Object(map)) => Some(map),
        Ok(_) => {
            tracing::warn!("extra_body 不是 JSON 对象，不发这些参数");
            None
        }
        Err(error) => {
            tracing::warn!(%error, "extra_body 不是合法 JSON，不发这些参数");
            None
        }
    }
}

/// 把额外参数并进请求体：同名键覆盖内置值。
fn merge_extra_body(
    body: &mut serde_json::Value,
    extra: &serde_json::Map<String, serde_json::Value>,
) {
    if let Some(fields) = body.as_object_mut() {
        for (key, value) in extra {
            fields.insert(key.clone(), value.clone());
        }
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p qingjian-predict && cargo clippy -p qingjian-predict --all-targets -- -D warnings`
Expected: 全部 PASS，clippy 干净。

- [ ] **Step 5: Commit**

```bash
git add crates/qingjian-predict/src/config.rs crates/qingjian-predict/src/chat_client.rs
git commit -m "feat(predict): 请求体支持 extra_body 额外参数，同名键覆盖内置值"
```

---

### Task 2: 配置模板与 crate 笔记

**Files:**
- Modify: `crates/qingjian-platform/src/config/mod.rs`（模板 `[predict]` 段 + `template_parses_to_defaults` 测试）
- Modify: `docs/notes/crate-notes.md`（predict 一节）

**Interfaces:**
- Consumes: Task 1 的 `PredictConfig.extra_body`（默认空串）。
- Produces: 新配置文件模板含 `extra_body = ""`；模板解析回默认值的断言。

- [ ] **Step 1: 模板加字段**

`crates/qingjian-platform/src/config/mod.rs` 模板 `[predict]` 段，`system_prompt = ""` 行后、`# 密钥：…` 注释前插：

```toml
# 额外请求参数：JSON 对象文本，原样合并进发往接口的请求体，同名键覆盖内置值。
# 比如智谱关思考可填 {"thinking": {"type": "disabled"}}；留空不发
extra_body = ""
```

- [ ] **Step 2: 模板测试补断言**

`template_parses_to_defaults` 里 `assert_eq!(config.predict.system_prompt, "");` 后加：

```rust
        assert_eq!(config.predict.extra_body, "");
```

- [ ] **Step 3: 跑测试**

Run: `cargo test -p qingjian-platform template`
Expected: PASS（先跑一次确认没写错键名）。

- [ ] **Step 4: crate-notes 同步**

`docs/notes/crate-notes.md` `## crates/qingjian-predict` 一节，`[predict] system_prompt` 那条之后加一条：

```markdown
- `[predict] extra_body` 是 JSON 对象文本：`ChatClient::new` 解析成 `serde_json::Map`（空串不发，不是对象记一条警告后整个忽略），`chat` 构建完请求体（含 `ThinkingSwitch` 的智谱改写）后合并，同名键覆盖内置值；测试连接走同一 `chat` 自动生效。设计见 `docs/design/extra-body.md`。
```

- [ ] **Step 5: Commit**

```bash
git add crates/qingjian-platform/src/config/mod.rs docs/notes/crate-notes.md
git commit -m "feat(platform): 配置模板加 [predict] extra_body"
```

---

### Task 3: macOS 云服务页 UI 与保存校验

**Files:**
- Modify: `apps/macos/Cargo.toml`（加 serde_json 依赖）
- Modify: `apps/macos/src/preferences/setting/mod.rs`（`ExtraBody` 变体、tag 58 双向映射、round-trip 测试）
- Modify: `apps/macos/src/host/settings.rs`（保存校验分支）
- Modify: `apps/macos/src/preferences/pages/cloud.rs`（UI 行、note、sync）
- Modify: `docs/user/cloud/index.md`（用户文档）

**Interfaces:**
- Consumes: Task 1 的 `PredictConfig.extra_body: String`；既有的 `row_text_view(layout, mtm, title, setting, target, height) -> Retained<NSTextView>`（失焦自动以 `SettingValue::Text` 回调 `changed:`，同 `SystemPrompt`）与 `self.apply_config(false)`（match 末尾统一调用，写完配置即重载 Predictor）。
- Produces: `Setting::ExtraBody`（tag 58）；`[predict] extra_body` 的落盘路径。

- [ ] **Step 1: 加依赖**

`apps/macos/Cargo.toml` `[dependencies]` 末尾（`toml_edit.workspace = true` 后）加一行：

```toml
serde_json.workspace = true
```

- [ ] **Step 2: Setting 枚举加变体与 tag**

`apps/macos/src/preferences/setting/mod.rs`：

`SystemPrompt` 变体后加：

```rust
    /// `[predict] extra_body`，「额外参数」多行文本框：失焦经 `textDidEndEditing:` 写回，同 SystemPrompt。
    ExtraBody,
```

`tag()` 里 `Self::SystemPrompt => 57,` 后加：

```rust
            Self::ExtraBody => 58,
```

`from_tag` 里 `57 => Self::SystemPrompt,` 后加：

```rust
            58 => Self::ExtraBody,
```

`tags_round_trip` 测试的数组里 `Setting::SystemPrompt,` 后加 `Setting::ExtraBody,`。

- [ ] **Step 3: 保存校验**

`apps/macos/src/host/settings.rs`，`(Setting::SystemPrompt, SettingValue::Text(text))` 分支后加：

```rust
            // 额外参数：留空是清空（合法）；非空时得是 JSON 对象，不是就不落盘并在状态行说清楚
            (Setting::ExtraBody, SettingValue::Text(text)) => {
                let text = text.trim();
                if text != config.predict.extra_body {
                    if text.is_empty()
                        || serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(text)
                            .is_ok()
                    {
                        self.settings.set_value("predict", "extra_body", text);
                    } else {
                        self.preferences
                            .set_status("额外参数没有保存：不是合法的 JSON 对象");
                        return;
                    }
                }
            }
```

（不落盘的分支 `return` 跳过末尾的 `self.apply_config(false)`，与 ApiKey 的错误分支同款。）

- [ ] **Step 4: 云服务页 UI**

`apps/macos/src/preferences/pages/cloud.rs`：

`PROMPT_HEIGHT` 常量后加：

```rust
/// 「额外参数」多行框的高度：一两条参数看得全，更长出滚动条。
const EXTRA_BODY_HEIGHT: f64 = 72.0;
```

`CloudPage` 结构体 `model` 字段后加：

```rust
    /// 额外请求参数（多行 JSON，失焦保存）。
    extra_body: Retained<NSTextView>,
```

`build` 里 `let model = …` 与 `row_control(layout, mtm, "模型", &model);` 之后、`let prompt = …` 之前插：

```rust
        let extra_body = row_text_view(
            layout,
            mtm,
            "额外参数",
            Setting::ExtraBody,
            target,
            EXTRA_BODY_HEIGHT,
        );
        note(
            layout,
            mtm,
            "原样合并进请求体的 JSON，同名键覆盖内置值，比如智谱关思考填 {\"thinking\": {\"type\": \"disabled\"}}。留空不发。文本框失焦时保存。",
        );
```

`Self { … }` 里 `model,` 后加 `extra_body,`。

`sync` 里 `self.model.setEnabled(cloud);` 后加 `self.extra_body.setEditable(cloud);`；`self.model.setStringValue(…)` 后加：

```rust
        self.extra_body
            .setString(&NSString::from_str(&config.predict.extra_body));
```

- [ ] **Step 5: 用户文档**

`docs/user/cloud/index.md`，「自定义联想提示词」一节后加：

```markdown
## 额外请求参数

「云服务」页的「额外参数」（配置文件 `[predict]` 的 `extra_body`）可填一段 JSON，会**原样合并**进发送给服务商的请求体；与内置参数同名时以这里填的为准。留空则不发送。
例如智谱关闭思考：`{"thinking": {"type": "disabled"}}`。不是合法 JSON 对象时不会保存；直接改配置文件填错时该参数不生效，日志里有说明。
```

- [ ] **Step 6: 编译与测试**

Run: `cargo clippy -p qingjian-macos --all-targets -- -D warnings && cargo test -p qingjian-macos`
Expected: clippy 干净，测试全过（含 `tags_round_trip`）。

- [ ] **Step 7: Commit**

```bash
git add apps/macos/Cargo.toml apps/macos/Cargo.lock apps/macos/src/preferences/setting/mod.rs apps/macos/src/host/settings.rs apps/macos/src/preferences/pages/cloud.rs docs/user/cloud/index.md
git commit -m "feat(macos): 云服务页增加「额外参数」JSON 输入框"
```

---

### Task 4: 全量验证

**Files:** 无新增改动（除非验证发现问题）。

**Interfaces:**
- Consumes: Task 1–3 的全部产物。
- Produces: 全 workspace 测试与 clippy 通过的确认。

- [ ] **Step 1: 全 workspace 测试**

Run: `cargo test`
Expected: 全部 PASS（pre-push 钩子同款命令，提前跑免得推时才发现）。

- [ ] **Step 2: 全量 clippy 与格式**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all --check`
Expected: 干净。

- [ ] **Step 3: 真机验证（人工，提醒用户）**

改 UI 行为按仓库约定要真机验：`apps/macos/scripts/bundle.sh --install` 装到本机，云服务页填 `{"thinking": {"type": "disabled"}}` 失焦保存 → 确认配置文件写入、状态行无报错；填 `not json` → 确认状态行提示且配置不变；点「测试连接」确认请求带上了额外参数。此步交由用户完成，代理不代跑。

- [ ] **Step 4: 如有修复则提交**

仅当 Step 1–2 发现问题并修复后执行：

```bash
git add -A
git commit -m "fix(macos): 额外参数联调修复"
```
