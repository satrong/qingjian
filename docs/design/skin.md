# 自定义皮肤（2026-10-03）

## 起因

`qingjian-render` 的 `Theme` 已经收拢了全部可视参数（字体、配色、间距、圆角），但只有 `light()` / `dark()` 两个硬编码构造，
文件头就写着「将来从 TOML 读」；[rendering.md](rendering.md) 定方向时也已写明「主题文件 TOML，随包给内置主题，用户目录可加」，
并把它列为渲染器落地后的下一步。本文把这一条落成具体方案。

**范围（2026-10-03 定）**：第一版只做配色 + 字号 + 间距圆角，一份皮肤文件自带 light / dark 两套，
覆盖 macOS 与 Windows（Linux 候选面板由 Fcitx5 自绘，无承载面），皮肤从本地文件来、设置页里选。
不做图片 / 动图 / 花边，不做在线市场，不做 HTML/CSS（`candidate-ui.md` 已排除）。

## 皮肤文件

放在配置目录的 `themes/` 下，一文件一皮肤，TOML：

| 平台 | 目录 |
|---|---|
| macOS | `~/Library/Application Support/Qingjian/themes/` |
| Windows | `%APPDATA%\Qingjian\themes\` |
| Linux | `$XDG_CONFIG_HOME/qingjian/themes/`（本版不消费，只建目录） |

```toml
[skin]
name = "夜航"          # 缺省取文件名；设置页显示用
author = "…"           # 可选
description = "…"      # 可选

# 两个分节都是可选的，缺哪个回退内置 Theme::light() / Theme::dark()；
# 分节里缺哪个字段回退该分节的默认值——用户可以只改颜色不动字号。
[skin.dark]
background = "#1E1E1EFF"
highlight  = "#244C24FF"
text       = "#FFFFFFFF"
gloss      = "#C8C8C8FF"
corner_radius = 12.0
max_rows = 7

[skin.dark.text_font]
size = 17.0
line_height = 20.0

[skin.light]
# …
```

- **颜色写 `#RRGGBB` 或 `#RRGGBBAA` 十六进制字符串**，与设计工具的输出一致；非法值记警告并回退默认，不让一个错色值打不开候选窗。
- **字族不在皮肤里**。`FontSpec` 刻意只含 `size` / `line_height`，字族走既有的 `[general] font`（mac 侧 CoreText 查文件、Windows 侧 DirectWrite 查文件，已验过热切换）。
  这样字体与皮肤是两条正交的配置，不打破现有设计。
- **`text_gamma` 默认跟着分节走**（浅 0.85 / 深 0.75），一般不该由皮肤改；开放它但不推荐。

### 与 `[general] theme` 的关系

`theme` 仍是 `system` / `light` / `dark` 三档，语义是「**用哪一套配色**」：
`system` 按系统深浅选 dark 或 light 分节，另两档固定。皮肤是**叠加在这之上的覆盖层**——
`[general] skin`（字符串，皮肤名，空 = 不用皮肤）非空时，按 `theme` 选出的分节再与皮肤文件合并。

这样 `ThemeMode` 枚举、`RouterConfig.theme`、菜单与偏好设置里已有的三档选择全都不动，
皮肤文件缺分节 / 缺字段时逐层回退到内置默认，配置里没有 `skin` 键的老配置文件照常工作。

## 代码落点

### `crates/qingjian-render`：解析与合并

- `Color` 加 `from_hex` / `to_hex`（`#RRGGBB` / `#RRGGBBAA`，大小写都认、`#RGB` 短写不认）与自定义 `Serialize`/`Deserialize`（serde 表示即十六进制字符串）；
  `FontSpec` 加 `Deserialize`（`deny_unknown_fields`，拼错键名整份失败）。`Theme` / `Palette` 不进文件格式、不加 serde——文件里的覆盖层是 `ThemePatch`。
- 新增 `theme/patch.rs`：`ThemePatch`（全字段 `Option` 的部分覆盖 + `apply`），颜色字段平铺、`deserialize_with` 容错——
  色值格式非法只回退该字段并 `tracing::warn!`，其余字段照常应用。
- 新增 `theme/file.rs`：`ThemeFile`（`name` / `author` / `description` + `light` / `dark` 两个 `Option<ThemePatch>`）、
  `from_toml`（根表必须是 `[skin]`，结构错误返回带行号的 `toml::de::Error`）与 `resolve(dark: bool) -> Theme`
  ——以 `Theme::dark()` / `Theme::light()` 为底，逐字段覆盖。
- 单测钉住：部分覆盖、缺分节回退、非法色值回退且不 panic、`#RGB` 短写不接受、结构错误（缺 `[skin]` / 键名拼错 / 类型不对）报错。
- `examples/preview.rs` 加 `--skin <path>`：离线按皮肤出全套 PNG，迭代配色不用起壳。

皮肤文件本质是渲染数据，解析放在 render 里最自然（它平台无关，不违反架构约束）；
`qingjian-platform` 只负责「配置里那个字符串」与目录路径。

### `crates/qingjian-platform`：配置

- `GeneralConfig` 加 `pub skin: String`（`#[serde(default)]`，空 = 不用皮肤）。
- 三平台的主题目录路径各自加一个函数（mac `apps/macos/src/app/paths.rs`、win `crates/qingjian-platform/src/dirs.rs`、linux `apps/linux/server/src/paths.rs`）。
- 随包内置皮肤放资源根下 `themes/`，与用户目录的并列列出、同名时用户目录优先。

### 渲染调用点：从 `dark: bool` 换成解析好的 `Theme`

| 位置 | 现状 | 改动 |
|---|---|---|
| `apps/macos/src/candidates/bitmap/mod.rs:124` | `if dark { Theme::dark() } else { Theme::light() }` | 换成配置解析出的 `Theme` |
| `apps/macos/src/host/config/mod.rs:44` | `set_theme(ThemeMode)` | 一并下发皮肤名，重新解析 |
| `apps/windows/server/src/ui/painter/mod.rs:119` | `fn theme(dark: bool)` | 改为吃解析好的 `Theme` |
| `apps/windows/server/src/ui/candidates/mod.rs:195` `sync_theme()` | 只在 DPI / dark 变时重建 | 失效条件加「皮肤名变了」 |
| `apps/windows/server/src/ui/status/mod.rs:160` | 同上 | 与候选窗同步，共用一个 `Theme` |
| `apps/windows/server/src/dispatch/config.rs` `RenderSettings` | `{ renderer, font }` | 加皮肤字段，跨线程送 UI 线程 |

状态条与候选窗共用一次 `dark` 判定，跟着一起换，不需要单独的皮肤开关。

### 热加载

`host/config/watch.rs`（mac，每秒 stat mtime）与 `dispatch/reload/mod.rs`（win，`CONFIG_POLL_INTERVAL`）
现在只盯 `config.toml`。把皮肤文件路径与 `themes/` 目录 mtime 一并纳入 stat；
只改皮肤文件不碰 `config.toml` 也要能生效，否则「改皮肤看效果」要手动 touch 一次配置，太别扭。

## 与平台原生退路的关系

皮肤**只在青简渲染器下生效**。macOS 的 AppKit 路径（`candidates/theme.rs`，`NSFont`/`NSColor`）与
Windows 的 GDI 路径（`ui/candidates/theme/`，`HFONT`/`COLORREF`）是两套独立结构，不为它们做降级映射：
选了皮肤就要求 `renderer = "qingjian"`，设置页选皮肤时提示「皮肤只在青简渲染器下生效」。
两套旧路径本来就计划稳定一版后删（`rendering.md:111`），皮肤不值得为它加代码。

## 设置界面

- **macOS**：`preferences/setting/mod.rs` 加 `Setting::Skin`，**同时进 `tags_round_trip` 测试的 `all` 数组**（漏了会挂）。
  控件照抄 `preferences/font_picker/` 的「按钮 + `NSPopover` + 搜索框 + `NSTableView`」，逐行用皮肤自己的配色渲染名字当预览。
  归「候选窗口」页，放不下再拆（`candidate-ui.md:302`）。
- **Windows**：`panel/pages/candidates.rs` 加 `Message::Skin` 分支 + `select(...)`，`panel/message.rs` 加消息。

## 前置项

**opsz 补丁按字号分键（2026-10-03 完成，fork 提交待推送钉 rev）**（`rendering.md:93`）：
fork 的 `font_cache` 改按（字重，光学字号）分键、`set_optical_size` 不再整表清缓存；
渲染器 `shape` 前按 `style.points` 设 opsz（<20 pt → 17，≥20 pt → 字号本身），倍数变化时清字形栅格缓存。
详见 `rendering.md` 光学字号一节。

## 分期

1. **数据模型 + 文件解析（2026-10-03 完成）**：`Color` 十六进制与 serde、`ThemePatch` / `ThemeFile` 解析合并、
   `examples/preview.rs --skin <path>` 离线出 PNG 迭代皮肤、上面那几条单测。
2. **macOS 打通（2026-10-03 完成）**：`[general] skin` 配置字段与模板、`candidates/skin.rs` 解析下发
   （`Host::apply_config` → `window.set_skin` → `BitmapPainter`，相同皮肤不重画）、tick 里 stat 皮肤文件与
   `themes/` 目录 mtime 热加载、分页 `max_rows` 跟皮肤走（仅青简渲染器）；`bundle.sh --install` 真机验待跑。
3. **Windows 打通（2026-10-03 完成）**：`RenderSettings` 加 `skin` 下发、`ConfigReload` 签名盯皮肤文件与
   `themes/` 目录、`Painter::configure` 字体或皮肤变了才重建、候选窗与状态条 `restyle()` 原地重画；
   真机验待跑，本机 windows-gnu 交叉检查覆盖 `platform` / `render` / `windows-settings`
   （server 卡在 `onig_sys` 缺 mingw——环境问题，见 `todo.md`）。分页仍按 `page_size`
   （设计落点表无此项），`max_rows` 未接皮肤，与 mac 有已知差异。
4. **设置页 + 内置皮肤（2026-10-03 完成）**：mac `Setting::Skin`（tag 61，进 `tags_round_trip`）+
   `preferences/skin_picker/`（照 `font_picker` 的 popover + 搜索 + 列表，每行浅 / 深两半用皮肤配色渲染名字当预览）；
   win `Message::Skin` 下拉（第 0 项「默认」）。仓库根 `themes/` 夜航 / 竹青 / 蜜柑三套（`preview --skin` 逐色核对）
   进 `bundle.sh` 与 `qingjian.iss`；`Config::write_themes_readme_if_missing` 首次运行在配置旁 `themes/`
   写 README（mac 设置、win Server、win 设置三处接入）；真机验待跑。
5. **文档（2026-10-03 完成）**：`crate-notes.md` render / platform / macos / windows 四节、
   `docs/user/settings/preferences.md` 候选窗口行加皮肤、`rendering.md` 指过来（116 行）。

## 留到以后

- **背景透明度**：`Color` 已有 `a`，但背景 alpha 要与阴影合成、与分层窗口的预乘一起调，单列一期。
- **图片 / 动图 / 花边**：`rendering.md:112` 记过「渲染器输出就是一张位图，装饰只是多叠几层，不用换底子」——
  届时给 `Theme` 加图层字段与图片解码即可，不换底子；第一版不碰。
- **导入导出 / 在线市场**：涉及分享与许可。`docs/contributing.md:91` 现在写的是「主题文件部分暂不接受 PR，稳定一版后再开」，
  放开的时机与第三方皮肤的许可证一并定（根 `LICENSE` 是测试版保留所有权利，随包皮肤的授权要单独写清）。
- **Linux**：等 Server 自绘面板 / GNOME 位图那条线（`todo.md` 三节）。
