use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::{AppHandle, Runtime};

/// Builds the native application menu.
///
/// Entries are placeholders for now; later tickets replace each id with the
/// real editor command. Clipboard and window items use Tauri's predefined
/// items so they behave natively on every platform.
pub fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let open = MenuItemBuilder::with_id("file.open", "打开…")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;

    let file = SubmenuBuilder::new(app, "文件")
        .item(&open)
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

    let view = SubmenuBuilder::new(app, "视图")
        .item(&actual_size)
        .item(&zoom_in)
        .item(&zoom_out)
        .separator()
        .fullscreen_with_text("进入全屏")
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

    builder
        .items(&[&file, &edit, &view, &help])
        .build()
}
