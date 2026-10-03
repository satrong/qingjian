//! 候选窗口：自绘 NSPanel 与视图、一帧的数据（preedit / 行 / 页脚）、主题。

mod bitmap;
mod cloud_icon;
mod frame;
mod preedit;
mod row;
mod skin;
mod theme;
mod view;
mod window;

pub(crate) use bitmap::available_families;
pub use frame::Frame;
pub use preedit::Preedit;
pub use row::Row;
pub(crate) use skin::{SkinStamp, resolve as resolve_skin, stamp as skin_stamp};
pub use window::CandidateWindow;
