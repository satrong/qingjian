//! 皮肤：按 `[general] skin` 的名字在 `themes/` 找皮肤文件，解析成深浅两套渲染主题。
//!
//! 格式与逐层回退在 [`qingjian_render::ThemeFile`]；这里只管定位文件、读文本与失败时回退内置。
//! 皮肤只在青简渲染器下生效，GDI 绘制路径不接（见 `docs/design/skin.md`）。
//! 用户目录 `%APPDATA%\Qingjian\themes/` 优先，随包根 `themes/` 兜底。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use qingjian_platform::{dirs, resources};
use qingjian_render::{SkinThemes, ThemeFile};

/// 皮肤文件与 `themes/` 目录的修改时间签名（没有的那项为 `None`）。
/// 热加载每秒只 stat 这两下，签名变了才重新读盘解析。
pub(super) type SkinStamp = (Option<SystemTime>, Option<SystemTime>);

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

/// 定位皮肤文件：用户 `themes/` 优先，随包 `themes/` 兜底；都不存在为 `None`。
fn skin_file(name: &str, user: Option<&Path>, bundled: Option<&Path>) -> Option<PathBuf> {
    let file = file_name(name);
    let user = user.map(|dir| dir.join(&file));
    let bundled = bundled.map(|dir| dir.join(&file));
    user.into_iter().chain(bundled).find(|path| path.is_file())
}

/// 当前皮肤名的修改时间签名；空名或不合法的名字恒为 `(None, None)`（解析结果不会变，不用重读）。
///
/// 只签用户 `themes/`：随包皮肤只随安装升级变，那时进程会重启。
pub(super) fn stamp(name: &str) -> SkinStamp {
    stamp_in(name, dirs::themes_dir().as_deref())
}

/// [`stamp`] 的可注入版：测试拿临时目录跑，不碰 `%APPDATA%`。
pub(super) fn stamp_in(name: &str, user: Option<&Path>) -> SkinStamp {
    let name = name.trim();
    if !is_valid(name) {
        return (None, None);
    }
    let file = file_name(name);
    let mtime = |path: PathBuf| {
        std::fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
    };
    let file_time = user.map(|dir| dir.join(&file)).and_then(mtime);
    let dir_time = user.and_then(|dir| mtime(dir.to_path_buf()));
    (file_time, dir_time)
}

/// 按皮肤名解析成两套主题。名字为空、文件不存在或读 / 解析失败都回退内置并记一次警告
/// （调用方保证只有签名变了才调，警告不会每秒刷屏）。
pub(super) fn resolve(name: &str) -> SkinThemes {
    let bundled = resources::bundled_root().map(|root| root.join("themes"));
    resolve_in(name, dirs::themes_dir().as_deref(), bundled.as_deref())
}

/// [`resolve`] 的可注入版：测试拿临时目录跑，不碰 `%APPDATA%` 与 exe 相对路径。
pub(super) fn resolve_in(name: &str, user: Option<&Path>, bundled: Option<&Path>) -> SkinThemes {
    let name = name.trim();
    if name.is_empty() {
        return SkinThemes::builtin();
    }
    if !is_valid(name) {
        tracing::warn!(name, "皮肤名含路径分隔符，用内置主题");
        return SkinThemes::builtin();
    }
    let Some(path) = skin_file(name, user, bundled) else {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// 每个测试一份独立临时目录，跑完即删。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "qingjian-skin-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_name_falls_back_to_builtin() {
        let dir = temp_dir("missing");
        assert_eq!(resolve_in("", Some(&dir), None), SkinThemes::builtin());
        assert_eq!(resolve_in("nope", Some(&dir), None), SkinThemes::builtin());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn path_separators_are_rejected() {
        let dir = temp_dir("traversal");
        fs::write(dir.join("escape.toml"), "[skin]\n").unwrap();
        // 名字带分隔符一律拒绝，哪怕拼出来的路径存在也不读。
        assert_eq!(
            resolve_in("../escape", Some(&dir), None),
            SkinThemes::builtin()
        );
        assert_eq!(
            resolve_in("..\\escape", Some(&dir), None),
            SkinThemes::builtin()
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn user_dir_wins_over_bundled() {
        let user = temp_dir("user");
        let bundled = temp_dir("bundled");
        fs::write(user.join("demo.toml"), "[skin.light]\nmax_rows = 5\n").unwrap();
        fs::write(bundled.join("demo.toml"), "[skin.light]\nmax_rows = 7\n").unwrap();
        let themes = resolve_in("demo", Some(&user), Some(&bundled));
        assert_eq!(themes.light.max_rows, 5);
        fs::remove_dir_all(&user).unwrap();
        fs::remove_dir_all(&bundled).unwrap();
    }

    #[test]
    fn bundled_file_is_used_when_user_file_missing() {
        let user = temp_dir("user2");
        let bundled = temp_dir("bundled2");
        fs::write(bundled.join("demo.toml"), "[skin.light]\nmax_rows = 7\n").unwrap();
        let themes = resolve_in("demo", Some(&user), Some(&bundled));
        assert_eq!(themes.light.max_rows, 7);
        fs::remove_dir_all(&user).unwrap();
        fs::remove_dir_all(&bundled).unwrap();
    }

    #[test]
    fn structural_error_falls_back_to_builtin() {
        let dir = temp_dir("broken");
        fs::write(dir.join("bad.toml"), "not_a_skin_table = 1\n").unwrap();
        assert_eq!(resolve_in("bad", Some(&dir), None), SkinThemes::builtin());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stamp_tracks_file_and_dir_mtime() {
        let dir = temp_dir("stamp");
        let empty = stamp_in("demo", Some(&dir));
        assert_eq!(empty.0, None);
        assert!(empty.1.is_some());
        fs::write(dir.join("demo.toml"), "[skin]\n").unwrap();
        let created = stamp_in("demo", Some(&dir));
        assert!(created.0.is_some());
        fs::write(dir.join("demo.toml"), "[skin]\nmax_rows = 5\n").unwrap();
        assert_ne!(stamp_in("demo", Some(&dir)), created);
        // 不合法的名恒为 (None, None)：解析结果不会变，不重读。
        assert_eq!(stamp_in("../x", Some(&dir)), (None, None));
        fs::remove_dir_all(&dir).unwrap();
    }
}
