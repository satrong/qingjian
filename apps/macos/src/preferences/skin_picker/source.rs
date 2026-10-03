//! 皮肤列表的数据源与代理：管下拉框的开合、按搜索框过滤、每行左右两半用该皮肤的浅色 / 深色
//! 配色渲染名字，用户选中一行就写配置并收起。

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSColor, NSControlTextEditingDelegate, NSFont, NSLineBreakMode, NSPopUpButton, NSPopover,
    NSSearchField, NSTableColumn, NSTableView, NSTableViewDataSource, NSTableViewDelegate,
    NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{
    NSIndexSet, NSInteger, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSRectEdge,
    NSSize, NSString,
};

use crate::candidates::SkinEntry;
use crate::preferences::setting::{Setting, SettingValue};
use qingjian_render::{Color, SkinThemes, Theme};

/// 行高：两半配色预览要放得下。
pub(super) const ROW: f64 = 26.0;

/// 「不用皮肤」那一行显示的名字。
const DEFAULT_LABEL: &str = "默认";

/// 列表里的一行：第 0 行永远是「默认」（`None`），其余指向 [`SkinState::skins`] 的下标。
type Row = Option<usize>;

define_class!(
    // SAFETY: 仅在主线程访问 AppKit 控件；回调里只读自己的快照。
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SkinState]
    pub(super) struct SkinListSource;

    unsafe impl NSObjectProtocol for SkinListSource {}
    unsafe impl NSControlTextEditingDelegate for SkinListSource {}
    unsafe impl NSTableViewDataSource for SkinListSource {
        #[unsafe(method(numberOfRowsInTableView:))]
        fn number_of_rows(&self, _table: &NSTableView) -> NSInteger {
            self.ivars().visible.borrow().len() as NSInteger
        }
    }
    unsafe impl NSTableViewDelegate for SkinListSource {
        #[unsafe(method_id(tableView:viewForTableColumn:row:))]
        fn view_for_row(
            &self,
            _table: &NSTableView,
            column: Option<&NSTableColumn>,
            row: NSInteger,
        ) -> Option<Retained<NSView>> {
            self.cell_view(column, row)
        }

        #[unsafe(method(tableViewSelectionDidChange:))]
        fn selection_changed(&self, _notification: &NSNotification) {
            if self.ivars().syncing.get() {
                return;
            }
            let Some(id) = self.selected_id() else {
                return;
            };
            self.close();
            self.show_current(&id);
            crate::host::with(|h| h.change_setting(Setting::Skin, SettingValue::Text(id)));
        }
    }
    impl SkinListSource {
        /// 搜索框每敲一个字（输入法上屏也算）都来一次：按内容过滤。
        #[unsafe(method(filterChanged:))]
        fn filter_changed(&self, sender: &NSSearchField) {
            let keep = self.selected_id();
            self.apply_filter(&sender.stringValue().to_string());
            self.select(keep.as_deref());
        }
    }
);

/// 选择器的状态：全部皮肤、过滤后可见的那些、控件引用，供回调独立读取。
pub(super) struct SkinState {
    /// 可选的皮肤（不含「默认」那一行），按显示名排序。
    skins: Vec<SkinEntry>,

    /// 过滤后显示的行：第 0 行永远是 `None`（默认）。
    visible: RefCell<Vec<Row>>,

    /// 配置里当前的皮肤 id（空为不用皮肤）。
    current: RefCell<String>,

    /// 页面上的按钮，显示当前皮肤名。
    button: RefCell<Option<Retained<NSPopUpButton>>>,

    /// 下拉框里的搜索框。
    search: RefCell<Option<Retained<NSSearchField>>>,

    /// 列表。
    table: RefCell<Option<Retained<NSTableView>>>,

    /// 装着列表的下拉框。
    popover: RefCell<Option<Retained<NSPopover>>>,

    /// 程序在同步选中行时为 true，此时选中变化不当作用户操作。
    syncing: Cell<bool>,
}

impl SkinListSource {
    pub(super) fn new(mtm: MainThreadMarker, skins: Vec<SkinEntry>) -> Retained<Self> {
        let visible = std::iter::once(None)
            .chain((0..skins.len()).map(Some))
            .collect();
        let this = mtm.alloc::<Self>().set_ivars(SkinState {
            skins,
            visible: RefCell::new(visible),
            current: RefCell::new(String::new()),
            button: RefCell::new(None),
            search: RefCell::new(None),
            table: RefCell::new(None),
            popover: RefCell::new(None),
            syncing: Cell::new(false),
        });
        unsafe { msg_send![super(this), init] }
    }

    pub(super) fn attach(
        &self,
        button: Retained<NSPopUpButton>,
        search: Retained<NSSearchField>,
        table: Retained<NSTableView>,
        popover: Retained<NSPopover>,
    ) {
        *self.ivars().button.borrow_mut() = Some(button);
        *self.ivars().search.borrow_mut() = Some(search);
        *self.ivars().table.borrow_mut() = Some(table);
        *self.ivars().popover.borrow_mut() = Some(popover);
    }

    /// 配置变了：记下当前皮肤，按钮显示它的名字，列表选中它。
    pub(super) fn set_current(&self, skin: &str) {
        *self.ivars().current.borrow_mut() = skin.trim().to_owned();
        self.show_current(skin);
        self.select((!skin.trim().is_empty()).then_some(skin.trim()));
    }

    /// 用户点了按钮：清空过滤、选中当前皮肤、弹出列表、焦点给搜索框。
    pub(super) fn open(&self) {
        let state = self.ivars();
        let (Some(button), Some(search), Some(popover)) = (
            state.button.borrow().clone(),
            state.search.borrow().clone(),
            state.popover.borrow().clone(),
        ) else {
            return;
        };
        if popover.isShown() {
            return;
        }
        search.setStringValue(&NSString::from_str(""));
        self.apply_filter("");
        let current = state.current.borrow().clone();
        self.select((!current.is_empty()).then_some(current.as_str()));
        popover.showRelativeToRect_ofView_preferredEdge(button.bounds(), &button, NSRectEdge::MaxY);
        if let Some(window) = search.window() {
            window.makeFirstResponder(Some(&search));
        }
    }

    fn close(&self) {
        if let Some(popover) = self.ivars().popover.borrow().as_ref()
            && popover.isShown()
        {
            popover.close();
        }
    }

    /// 按钮上显示当前皮肤名（空为「默认」）：按钮的菜单只有这一项，只当显示用。
    /// id 不在列表里（文件被删了 / 手改的配置）就原样显示。
    fn show_current(&self, skin: &str) {
        let state = self.ivars();
        let skin = skin.trim();
        let label = if skin.is_empty() {
            DEFAULT_LABEL
        } else {
            state
                .skins
                .iter()
                .find(|entry| entry.id == skin)
                .map_or(skin, |entry| entry.label.as_str())
        };
        if let Some(button) = state.button.borrow().as_ref() {
            button.removeAllItems();
            button.addItemWithTitle(&NSString::from_str(label));
            button.selectItemAtIndex(0);
        }
    }

    /// 选中某个皮肤（`None` 或没在列表里就选「默认」），不触发写配置。
    fn select(&self, skin: Option<&str>) {
        let state = self.ivars();
        let Some(table) = state.table.borrow().clone() else {
            return;
        };
        let row = skin
            .and_then(|skin| {
                state
                    .visible
                    .borrow()
                    .iter()
                    .position(|row| row.is_some_and(|index| state.skins[index].id == skin))
            })
            .unwrap_or(0);
        state.syncing.set(true);
        table.selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
        table.scrollRowToVisible(row as NSInteger);
        state.syncing.set(false);
    }

    /// 大小写不敏感的子串过滤（匹配 id 与显示名）；「默认」永远在第 0 行。
    fn apply_filter(&self, text: &str) {
        let state = self.ivars();
        let needle = text.trim().to_lowercase();
        let mut visible: Vec<Row> = vec![None];
        visible.extend(
            state
                .skins
                .iter()
                .enumerate()
                .filter(|(_, entry)| {
                    needle.is_empty()
                        || entry.label.to_lowercase().contains(&needle)
                        || entry.id.to_lowercase().contains(&needle)
                })
                .map(|(index, _)| Some(index)),
        );
        *state.visible.borrow_mut() = visible;
        if let Some(table) = state.table.borrow().as_ref() {
            state.syncing.set(true);
            table.reloadData();
            state.syncing.set(false);
        }
    }

    fn selected_id(&self) -> Option<String> {
        let state = self.ivars();
        let table = state.table.borrow();
        let row = usize::try_from(table.as_ref()?.selectedRow()).ok()?;
        match state.visible.borrow().get(row).copied().flatten() {
            None => Some(String::new()),
            Some(index) => Some(state.skins[index].id.clone()),
        }
    }

    fn cell_view(
        &self,
        column: Option<&NSTableColumn>,
        row: NSInteger,
    ) -> Option<Retained<NSView>> {
        let state = self.ivars();
        let row = usize::try_from(row).ok()?;
        let which = *state.visible.borrow().get(row)?;
        let column = column?;
        let mtm = MainThreadMarker::from(self);
        let width = column.width();
        let view = NSView::initWithFrame(
            mtm.alloc(),
            NSRect::new(NSPoint::ZERO, NSSize::new(width, ROW)),
        );
        // 第 0 行（默认）用内置两套配色；其余行用该皮肤解析出来的两套。
        let builtin = SkinThemes::builtin();
        let (light, dark) = match which {
            None => (builtin.get(false), builtin.get(true)),
            Some(index) => {
                let entry = &state.skins[index];
                (&entry.themes.light, &entry.themes.dark)
            }
        };
        let name = which.map_or(DEFAULT_LABEL, |index| state.skins[index].label.as_str());
        let half = width / 2.0;
        view.addSubview(&pane(
            mtm,
            NSRect::new(NSPoint::ZERO, NSSize::new(half - 0.5, ROW)),
            light,
            name,
        ));
        view.addSubview(&pane(
            mtm,
            NSRect::new(NSPoint::new(half + 0.5, 0.0), NSSize::new(half - 0.5, ROW)),
            dark,
            name,
        ));
        Some(view)
    }
}

/// 一半预览：皮肤自己的背景 + 文字色渲染名字。
fn pane(mtm: MainThreadMarker, frame: NSRect, theme: &Theme, name: &str) -> Retained<NSTextField> {
    let field = NSTextField::labelWithString(&NSString::from_str(name), mtm);
    field.setFrame(frame);
    field.setDrawsBackground(true);
    field.setBackgroundColor(Some(&ns_color(theme.colors.background)));
    field.setTextColor(Some(&ns_color(theme.colors.text)));
    field.setAlignment(NSTextAlignment::Center);
    field.setFont(Some(&NSFont::systemFontOfSize(13.0)));
    field.setUsesSingleLineMode(true);
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field
}

/// 渲染器的 `Color`（0–255）→ AppKit 的 `NSColor`。
fn ns_color(color: Color) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(color.r) / 255.0,
        f64::from(color.g) / 255.0,
        f64::from(color.b) / 255.0,
        f64::from(color.a) / 255.0,
    )
}
