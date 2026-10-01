# 云联想额外参数（2026-10-01）

云服务页新增「额外参数」：一段 JSON，原样合并进发往 LLM 的请求体。各厂商参数五花八门
（智谱 `thinking`、Anthropic 兼容层、代理网关的自定义字段……），不可能每家都做成配置项，
放开一个 JSON 口子最省事。

## 决定

### 存储与字段

- `PredictConfig`（`[predict]` 分节）新增 `extra_body: String`，默认空串；旧配置文件靠已有的
  `#[serde(default)]` 自动补缺，不用迁移。
- 值是 JSON 对象的**文本**，不做类型化结构——类型化就没法「随便配」了。

### 合并顺序与优先级

请求体在 `ChatClient::chat` 统一构建（联想、问字、翻译、测试连接共用），合并放这条链的**最后一步**：

1. 内置字段（model / messages / max_tokens / temperature / response_format）
2. `reasoning_effort`
3. `ThinkingSwitch::disable`（智谱自动改写 `thinking`）
4. **合并 `extra_body`，同名键用户 JSON 覆盖内置值**

用户 JSON 永远赢：规则一句话说得清（写了什么发什么），也保留了手动关思考、调温度这类
完全控制的能力。测试连接走同一个 `chat`，自动生效。

### 解析时机

`ChatClient::new` 时解析一次成 `Option<serde_json::Map<String, Value>>`，请求路径零解析开销；
解析失败或不是 JSON 对象记一条 `warn!` 日志后当 `None`——配置文件可以绕过 UI 手改，请求侧必须兜底，
但每次联想请求都刷警告太吵，所以只在构建时说一次。

### UI 与保存校验

- 云服务页「模型」与「联想提示词」之间加多行文本框（约 72 高），标签「额外参数」，失焦保存，
  云联想关闭时随其他云服务项一起置灰。
- 下方 note 说明：原样合并、同名键覆盖、智谱关思考的例子、留空不发。
- 保存时：留空或没变不动；`serde_json` 解析失败或不是对象，状态行提示「额外参数没有保存：
  不是合法的 JSON 对象」，不落盘；合法写 `predict.extra_body` 并触发重载。

## 不做的事

- 不做每个参数的独立输入框或 schema 校验（违背自由配置初衷）。
- 不改 `reasoning_effort` 与 `ThinkingSwitch` 的现有行为：默认 `none` 依旧自动关思考，
  `extra_body` 只是想绕开时的一条出路。
- Windows 设置程序与 Linux 暂不加对应 UI（TOML 手改可用），后续跟进。
