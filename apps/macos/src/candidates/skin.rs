//! 皮肤：按 `[general] skin` 的名字在 `themes/` 找皮肤文件，解析成深浅两套渲染主题。
//!
//! 格式与逐层回退在 [`qingjian_render::ThemeFile`]；这里只管定位文件、读文本与失败时回退内置。
//! 皮肤只在青简渲染器下生效，AppKit 绘制路径不接（见 `docs/design/skin.md`）。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

use qingjian_render::ThemeFile;

pub use qingjian_render::{SkinEntry, SkinThemes};

use crate::app::paths;

/// 随包 + 用户 `themes/` 里全部可解析的皮肤，按显示名排序；同 id 时用户目录覆盖随包的。
pub fn list() -> Vec<SkinEntry> {
    let mut entries = BTreeMap::new();
    let bundled = paths::resources_dir().ok().map(|dir| dir.join("themes"));
    collect(bundled.as_deref(), &mut entries);
    collect(paths::themes_dir().as_deref(), &mut entries);
    let mut entries: Vec<SkinEntry> = entries.into_values().collect();
    entries.sort_by(|a, b| a.label.cmp(&b.label));
    entries
}

/// 扫一个目录；读不了 / 解析不了的文件在这里记警告。
fn collect(dir: Option<&std::path::Path>, into: &mut BTreeMap<String, SkinEntry>) {
    let Some(dir) = dir else {
        return;
    };
    for (path, why) in ThemeFile::collect_themes(dir, into) {
        tracing::warn!(path = %path.display(), "皮肤文件{why}，不进列表");
    }
}

/// 皮肤文件与 `themes/` 目录的修改时间签名（没有的那项为 `None`）。
/// 热加载每秒只 stat 这两下，签名变了才重新读盘解析。
pub type SkinStamp = (Option<SystemTime>, Option<SystemTime>);

/// 皮肤名合法：非空、不含路径分隔符（配置里写成 `../../etc/passwd` 这类一律拒绝）。
fn is_valid(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\')
}

/// `夜航` → `夜航.toml`；写全扩展名的原样用。
fn file_name(name: &str) -> String {
    if name.ends_with(".toml") {
        name.to_owned()
    } else {
        format!("{name}.toml")
    }
}

/// 定位皮肤文件：用户 `themes/` 优先，随包 `Resources/themes/` 兜底；都不存在为 `None`。
fn skin_file(name: &str) -> Option<PathBuf> {
    let file = file_name(name);
    let user = paths::themes_dir().map(|dir| dir.join(&file));
    let bundled = paths::resources_dir()
        .ok()
        .map(|dir| dir.join("themes").join(&file));
    user.into_iter().chain(bundled).find(|path| path.is_file())
}

/// 当前皮肤名的修改时间签名；空名或不合法的名字恒为 `(None, None)`（解析结果不会变，不用重读）。
pub fn stamp(name: &str) -> SkinStamp {
    let name = name.trim();
    if !is_valid(name) {
        return (None, None);
    }
    let file = file_name(name);
    let dir = paths::themes_dir();
    let mtime = |path: PathBuf| {
        std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
    };
    let file_time = dir.as_ref().map(|dir| dir.join(&file)).and_then(mtime);
    let dir_time = dir.and_then(mtime);
    (file_time, dir_time)
}

/// 按皮肤名解析成两套主题。名字为空、文件不存在或读 / 解析失败都回退内置并记一次警告
/// （调用方保证只有签名变了才调，警告不会每秒刷屏）。
pub fn resolve(name: &str) -> SkinThemes {
    let name = name.trim();
    if name.is_empty() {
        return SkinThemes::builtin();
    }
    if !is_valid(name) {
        tracing::warn!(name, "皮肤名含路径分隔符，用内置主题");
        return SkinThemes::builtin();
    }
    let Some(path) = skin_file(name) else {
        tracing::warn!(name, "找不到皮肤文件，用内置主题");
        return SkinThemes::builtin();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        tracing::warn!(path = %path.display(), "皮肤文件读不了，用内置主题");
        return SkinThemes::builtin();
    };
    match ThemeFile::from_toml(&text) {
        Ok(skin) => skin.themes(),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "皮肤文件解析失败，用内置主题");
            SkinThemes::builtin()
        }
    }
}
