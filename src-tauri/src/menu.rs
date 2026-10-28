use serde::Deserialize;
use tauri::menu::{Menu, MenuBuilder, MenuItem, MenuItemBuilder, MenuItemKind, SubmenuBuilder};
use tauri::{AppHandle, Runtime};

use crate::commands::{CommandError, CommandErrorCode};

/// Menu ids forwarded to the frontend. The workspace computes whether each
/// item can act and pushes that availability back through `set_menu_state`.
pub const FILE_OPEN: &str = "file.open";
pub const FILE_PRINT: &str = "file.print";
pub const FILE_EXPORT_PAGE: &str = "file.export_page";
pub const FILE_SAVE: &str = "file.save";
pub const FILE_SAVE_AS: &str = "file.save_as";
pub const APP_SETTINGS: &str = "app.settings";
pub const EDIT_UNDO: &str = "edit.undo";
pub const EDIT_REDO: &str = "edit.redo";
pub const VIEW_ACTUAL_SIZE: &str = "view.actual_size";
pub const VIEW_ZOOM_IN: &str = "view.zoom_in";
pub const VIEW_ZOOM_OUT: &str = "view.zoom_out";
pub const VIEW_FIT_WIDTH: &str = "view.fit_width";
pub const VIEW_FIT_PAGE: &str = "view.fit_page";
pub const VIEW_FULLSCREEN: &str = "view.fullscreen";
pub const PAGE_PREVIOUS: &str = "page.prev";
pub const PAGE_NEXT: &str = "page.next";

/// Undo/redo act on the document's Edit Session, not on the focused control's
/// text undo. The frontend releases these accelerators while a text control
/// has focus so the platform's native editing undo keeps working.
pub const UNDO_ACCELERATOR: &str = "CmdOrCtrl+Z";
/// One accelerator per item: Windows Ctrl+Y is handled by the webview keydown
/// fallback instead of a second (conflicting) accelerator on this item.
pub const REDO_ACCELERATOR: &str = "CmdOrCtrl+Shift+Z";
/// PageUp/PageDown follow the single-page viewer convention. They are released
/// while a text control has focus so the key keeps its native meaning there.
pub const PREVIOUS_PAGE_ACCELERATOR: &str = "PageUp";
pub const NEXT_PAGE_ACCELERATOR: &str = "PageDown";

/// The platform's fullscreen shortcut. Fullscreen is a custom menu item so the
/// frontend can restore the page and zoom it had before entering it.
#[cfg(target_os = "macos")]
const FULLSCREEN_ACCELERATOR: &str = "Ctrl+Command+F";
#[cfg(not(target_os = "macos"))]
const FULLSCREEN_ACCELERATOR: &str = "F11";

/// Items whose enabled state the workspace owns. Any other id in a
/// `set_menu_state` snapshot is rejected before any item is touched.
pub const STATEFUL_MENU_IDS: [&str; 16] = [
    FILE_OPEN,
    FILE_PRINT,
    FILE_EXPORT_PAGE,
    FILE_SAVE,
    FILE_SAVE_AS,
    APP_SETTINGS,
    EDIT_UNDO,
    EDIT_REDO,
    VIEW_ACTUAL_SIZE,
    VIEW_ZOOM_IN,
    VIEW_ZOOM_OUT,
    VIEW_FIT_WIDTH,
    VIEW_FIT_PAGE,
    VIEW_FULLSCREEN,
    PAGE_PREVIOUS,
    PAGE_NEXT,
];

/// The accelerator an item carries when it can act.
///
/// Returning `None` means the item never has one. Disabled items have their
/// accelerator removed by `set_menu_state`, so a key is never swallowed by an
/// item that cannot run (important on Windows, where a disabled menu item's
/// accelerator stays registered).
pub fn accelerator_for(id: &str) -> Option<&'static str> {
    match id {
        FILE_OPEN => Some("CmdOrCtrl+O"),
        FILE_PRINT => Some("CmdOrCtrl+P"),
        FILE_EXPORT_PAGE => Some("CmdOrCtrl+Shift+E"),
        FILE_SAVE => Some("CmdOrCtrl+S"),
        FILE_SAVE_AS => Some("CmdOrCtrl+Shift+S"),
        APP_SETTINGS => Some("CmdOrCtrl+,"),
        EDIT_UNDO => Some(UNDO_ACCELERATOR),
        EDIT_REDO => Some(REDO_ACCELERATOR),
        VIEW_ACTUAL_SIZE => Some("CmdOrCtrl+0"),
        VIEW_ZOOM_IN => Some("CmdOrCtrl+="),
        VIEW_ZOOM_OUT => Some("CmdOrCtrl+-"),
        VIEW_FIT_WIDTH => Some("CmdOrCtrl+1"),
        VIEW_FIT_PAGE => Some("CmdOrCtrl+2"),
        VIEW_FULLSCREEN => Some(FULLSCREEN_ACCELERATOR),
        PAGE_PREVIOUS => Some(PREVIOUS_PAGE_ACCELERATOR),
        PAGE_NEXT => Some(NEXT_PAGE_ACCELERATOR),
        _ => None,
    }
}

/// Whether an item starts enabled before the workspace pushes its first
/// availability snapshot. Document-dependent items start disabled; the
/// workspace enables them once a Document is open.
pub fn initial_enabled(id: &str) -> bool {
    matches!(id, FILE_OPEN | APP_SETTINGS | VIEW_FULLSCREEN)
}

/// Builds one custom menu item from the shared id/accelerator table.
fn item<R: Runtime>(app: &AppHandle<R>, id: &str, label: &str) -> tauri::Result<MenuItem<R>> {
    let mut builder = MenuItemBuilder::with_id(id, label).enabled(initial_enabled(id));
    if let Some(accelerator) = accelerator_for(id) {
        builder = builder.accelerator(accelerator);
    }
    builder.build(app)
}

/// One menu item's enabled flag in a workspace availability snapshot.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuItemState {
    pub id: String,
    pub enabled: bool,
}

/// The full availability snapshot pushed by the workspace.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuState {
    pub items: Vec<MenuItemState>,
}

fn validate_menu_state(state: &MenuState) -> Result<(), CommandError> {
    for item in &state.items {
        if !STATEFUL_MENU_IDS.contains(&item.id.as_str()) {
            return Err(CommandError::new(
                CommandErrorCode::InvalidArgument,
                format!("unknown menu item id {}", item.id),
            ));
        }
    }
    Ok(())
}

fn menu_error(message: impl Into<String>) -> CommandError {
    CommandError::new(CommandErrorCode::Internal, message)
}

/// The accelerator to apply to a stateful item for a given availability.
///
/// `Some(Some(_))` enables an accelerator, `Some(None)` releases it, and
/// `None` means the item never carries one. Re-enabling restores the same
/// accelerator the menu was built with.
fn accelerator_change(id: &str, enabled: bool) -> Option<Option<&'static str>> {
    accelerator_for(id).map(|accelerator| if enabled { Some(accelerator) } else { None })
}

/// Finds one of the workspace's custom items anywhere in the menu tree.
///
/// `Menu::get` only searches the menu bar's direct children, which are the
/// 文件/编辑/视图/帮助 submenus. The stateful items themselves live one level
/// below, so this walks the submenus instead of treating every snapshot item
/// as missing.
fn find_item<R: Runtime>(menu: &Menu<R>, id: &str) -> Option<MenuItem<R>> {
    fn find_in<R: Runtime>(items: &[MenuItemKind<R>], id: &str) -> Option<MenuItem<R>> {
        for kind in items {
            match kind {
                MenuItemKind::MenuItem(item) if item.id().as_ref() == id => {
                    return Some(item.clone());
                }
                MenuItemKind::Submenu(submenu) => {
                    if let Ok(items) = submenu.items() {
                        if let Some(item) = find_in(&items, id) {
                            return Some(item);
                        }
                    }
                }
                _ => {}
            }
        }
        None
    }

    menu.items().ok().and_then(|items| find_in(&items, id))
}

/// Applies the workspace's availability snapshot to the native menu.
///
/// A disabled item also loses its accelerator: the key must reach the focused
/// text control, the page scroller or the platform's own handler instead of
/// firing a command that cannot run.
#[tauri::command]
pub fn set_menu_state(app: AppHandle, state: MenuState) -> Result<(), CommandError> {
    validate_menu_state(&state)?;
    let menu = app
        .menu()
        .ok_or_else(|| menu_error("the application has no menu"))?;
    for item in &state.items {
        let menu_item = find_item(&menu, &item.id)
            .ok_or_else(|| menu_error(format!("menu item {} is missing", item.id)))?;
        menu_item
            .set_enabled(item.enabled)
            .map_err(|error| menu_error(format!("could not update {}: {error}", item.id)))?;
        if let Some(accelerator) = accelerator_change(&item.id, item.enabled) {
            menu_item.set_accelerator(accelerator).map_err(|error| {
                menu_error(format!(
                    "could not update the accelerator of {}: {error}",
                    item.id
                ))
            })?;
        }
    }
    Ok(())
}

/// Builds the native application menu.
///
/// Menu ids are forwarded to the focused window and handled by the frontend.
/// Clipboard and window items use Tauri's predefined items so they behave
/// natively on every platform. Undo/redo are custom because they act on the
/// document's Edit Session rather than the focused control.
pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let open = item(app, FILE_OPEN, "打开…")?;
    let print = item(app, FILE_PRINT, "打印…")?;
    let export_page = item(app, FILE_EXPORT_PAGE, "导出当前页…")?;
    let save = item(app, FILE_SAVE, "保存")?;
    let save_as = item(app, FILE_SAVE_AS, "另存为…")?;
    let settings = item(app, APP_SETTINGS, "设置…")?;

    #[cfg(target_os = "macos")]
    let file_builder = SubmenuBuilder::new(app, "文件")
        .item(&open)
        .separator()
        .item(&print)
        .item(&export_page);
    #[cfg(not(target_os = "macos"))]
    let file_builder = SubmenuBuilder::new(app, "文件")
        .item(&open)
        .item(&settings)
        .separator()
        .item(&print)
        .item(&export_page);
    let file = file_builder
        .separator()
        .item(&save)
        .item(&save_as)
        .separator()
        .close_window_with_text("关闭窗口")
        .quit_with_text("退出 PDFX")
        .build()?;

    let undo = item(app, EDIT_UNDO, "撤销")?;
    let redo = item(app, EDIT_REDO, "重做")?;
    let edit = SubmenuBuilder::new(app, "编辑")
        .item(&undo)
        .item(&redo)
        .separator()
        .cut_with_text("剪切")
        .copy_with_text("复制")
        .paste_with_text("粘贴")
        .select_all_with_text("全选")
        .build()?;

    let actual_size = item(app, VIEW_ACTUAL_SIZE, "实际大小")?;
    let zoom_in = item(app, VIEW_ZOOM_IN, "放大")?;
    let zoom_out = item(app, VIEW_ZOOM_OUT, "缩小")?;
    let fit_width = item(app, VIEW_FIT_WIDTH, "适合宽度")?;
    let fit_page = item(app, VIEW_FIT_PAGE, "适合页面")?;
    let fullscreen = item(app, VIEW_FULLSCREEN, "全屏")?;
    let previous_page = item(app, PAGE_PREVIOUS, "上一页")?;
    let next_page = item(app, PAGE_NEXT, "下一页")?;

    let view = SubmenuBuilder::new(app, "视图")
        .item(&previous_page)
        .item(&next_page)
        .separator()
        .item(&actual_size)
        .item(&zoom_in)
        .item(&zoom_out)
        .separator()
        .item(&fit_width)
        .item(&fit_page)
        .separator()
        .item(&fullscreen)
        .build()?;

    let help = SubmenuBuilder::new(app, "帮助")
        .text("help.docs", "PDFX 帮助")
        .build()?;

    let mut builder = MenuBuilder::new(app);

    #[cfg(target_os = "macos")]
    {
        let app_menu = SubmenuBuilder::new(app, "PDFX")
            .about(None)
            .separator()
            .item(&settings)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .quit_with_text("退出 PDFX")
            .build()?;
        builder = builder.item(&app_menu);
    }

    builder.items(&[&file, &edit, &view, &help]).build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn undo_and_redo_use_the_standard_history_shortcuts() {
        assert_eq!(accelerator_for(EDIT_UNDO), Some("CmdOrCtrl+Z"));
        assert_eq!(accelerator_for(EDIT_REDO), Some("CmdOrCtrl+Shift+Z"));
    }

    #[test]
    fn redo_does_not_register_a_second_windows_accelerator() {
        // Windows Ctrl+Y is a webview keydown fallback; a duplicate menu
        // accelerator would make one key press run redo twice.
        let redo = accelerator_for(EDIT_REDO).expect("redo accelerator");
        assert!(!redo.to_ascii_lowercase().ends_with("+y"));
    }

    #[test]
    fn disabling_an_item_releases_its_accelerator_and_enabling_restores_it() {
        assert_eq!(accelerator_change(EDIT_UNDO, false), Some(None));
        assert_eq!(
            accelerator_change(EDIT_UNDO, true),
            Some(Some(UNDO_ACCELERATOR))
        );
        assert_eq!(accelerator_change(PAGE_NEXT, false), Some(None));
        assert_eq!(
            accelerator_change(PAGE_NEXT, true),
            Some(Some(NEXT_PAGE_ACCELERATOR))
        );
        // An item with no accelerator is left untouched in either state.
        assert_eq!(accelerator_change("help.docs", true), None);
        assert_eq!(accelerator_change("help.docs", false), None);
    }

    #[test]
    fn page_navigation_uses_page_up_and_page_down() {
        assert_eq!(accelerator_for(PAGE_PREVIOUS), Some("PageUp"));
        assert_eq!(accelerator_for(PAGE_NEXT), Some("PageDown"));
    }

    #[test]
    fn every_stateful_item_is_unique_and_has_an_accelerator() {
        let mut seen = HashSet::new();
        for id in STATEFUL_MENU_IDS {
            assert!(seen.insert(id), "duplicate menu id {id}");
            assert!(
                accelerator_for(id).is_some(),
                "menu item {id} has no accelerator"
            );
        }
        assert_eq!(seen.len(), STATEFUL_MENU_IDS.len());
    }

    #[test]
    fn no_two_stateful_items_share_an_accelerator() {
        // One key press must reach exactly one command. Undo/redo and page
        // navigation are the easy place to introduce a duplicate.
        let mut seen = HashSet::new();
        for id in STATEFUL_MENU_IDS {
            let accelerator = accelerator_for(id).expect("accelerator");
            assert!(
                seen.insert(accelerator),
                "duplicate accelerator {accelerator} on {id}"
            );
        }
    }

    #[test]
    fn document_actions_start_disabled_and_shell_actions_start_enabled() {
        assert!(initial_enabled(FILE_OPEN));
        assert!(initial_enabled(APP_SETTINGS));
        assert!(initial_enabled(VIEW_FULLSCREEN));
        for id in [
            FILE_PRINT,
            FILE_EXPORT_PAGE,
            FILE_SAVE,
            FILE_SAVE_AS,
            EDIT_UNDO,
            EDIT_REDO,
            VIEW_ACTUAL_SIZE,
            VIEW_ZOOM_IN,
            VIEW_ZOOM_OUT,
            VIEW_FIT_WIDTH,
            VIEW_FIT_PAGE,
            PAGE_PREVIOUS,
            PAGE_NEXT,
        ] {
            assert!(!initial_enabled(id), "{id} should start disabled");
        }
    }

    #[test]
    fn an_unknown_menu_id_is_rejected_before_any_item_changes() {
        let state = MenuState {
            items: vec![
                MenuItemState {
                    id: FILE_SAVE.to_owned(),
                    enabled: true,
                },
                MenuItemState {
                    id: "file.missing".to_owned(),
                    enabled: true,
                },
            ],
        };

        let error = validate_menu_state(&state).expect_err("unknown id");
        assert_eq!(error.code, CommandErrorCode::InvalidArgument);
    }

    #[test]
    fn the_menu_state_payload_uses_the_frontend_snapshot_shape() {
        // Mirrors `setMenuState` in `src/lib/backend.ts`.
        let state: MenuState = serde_json::from_str(
            r#"{"items":[{"id":"file.save","enabled":true},{"id":"edit.undo","enabled":false}]}"#,
        )
        .expect("deserialize the workspace snapshot");

        assert_eq!(state.items.len(), 2);
        assert_eq!(state.items[0].id, FILE_SAVE);
        assert!(state.items[0].enabled);
        assert_eq!(state.items[1].id, EDIT_UNDO);
        assert!(!state.items[1].enabled);
    }

    #[test]
    fn a_full_stateful_snapshot_is_accepted() {
        let state = MenuState {
            items: STATEFUL_MENU_IDS
                .iter()
                .map(|id| MenuItemState {
                    id: (*id).to_owned(),
                    enabled: true,
                })
                .collect(),
        };

        assert!(validate_menu_state(&state).is_ok());
    }
}
