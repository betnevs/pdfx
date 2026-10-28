use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Runtime};

/// The platform's fullscreen shortcut. Fullscreen is a custom menu item so the
/// frontend can restore the page and zoom it had before entering it.
#[cfg(target_os = "macos")]
const FULLSCREEN_ACCELERATOR: &str = "Ctrl+Command+F";
#[cfg(not(target_os = "macos"))]
const FULLSCREEN_ACCELERATOR: &str = "F11";

/// Builds the native application menu.
///
/// Menu ids are forwarded to the focused window and handled by the frontend.
/// Clipboard and window items use Tauri's predefined items so they behave
/// natively on every platform.
pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let open = MenuItemBuilder::with_id("file.open", "打开…")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    let settings = MenuItemBuilder::with_id("app.settings", "设置…")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;

    #[cfg(target_os = "macos")]
    let file_builder = SubmenuBuilder::new(app, "文件").item(&open);
    #[cfg(not(target_os = "macos"))]
    let file_builder = SubmenuBuilder::new(app, "文件").item(&open).item(&settings);
    let file = file_builder
        .separator()
        .close_window_with_text("关闭窗口")
        .quit_with_text("退出 PDFX")
        .build()?;

    let edit = SubmenuBuilder::new(app, "编辑")
        .undo_with_text("撤销")
        .redo_with_text("重做")
        .separator()
        .cut_with_text("剪切")
        .copy_with_text("复制")
        .paste_with_text("粘贴")
        .select_all_with_text("全选")
        .build()?;

    let actual_size = MenuItemBuilder::with_id("view.actual_size", "实际大小")
        .accelerator("CmdOrCtrl+0")
        .build(app)?;
    let zoom_in = MenuItemBuilder::with_id("view.zoom_in", "放大")
        .accelerator("CmdOrCtrl+=")
        .build(app)?;
    let zoom_out = MenuItemBuilder::with_id("view.zoom_out", "缩小")
        .accelerator("CmdOrCtrl+-")
        .build(app)?;
    let fit_width = MenuItemBuilder::with_id("view.fit_width", "适合宽度")
        .accelerator("CmdOrCtrl+1")
        .build(app)?;
    let fit_page = MenuItemBuilder::with_id("view.fit_page", "适合页面")
        .accelerator("CmdOrCtrl+2")
        .build(app)?;
    let fullscreen = MenuItemBuilder::with_id("view.fullscreen", "全屏")
        .accelerator(FULLSCREEN_ACCELERATOR)
        .build(app)?;

    let view = SubmenuBuilder::new(app, "视图")
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
