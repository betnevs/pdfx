pub mod commands;
pub mod engine;
mod menu;
pub mod pdfium_adapter;
pub mod text_edit;

use tauri::{Emitter, Manager};

pub use commands::{app_info, AppInfo};

/// Event emitted to the focused window when a native menu item is activated.
/// Keyboard accelerators attached to those menu items trigger the same event.
pub const MENU_EVENT: &str = "pdfx://menu";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .menu(menu::build_menu)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match app
                .webview_windows()
                .values()
                .find(|window| window.is_focused().unwrap_or(false))
            {
                Some(window) => {
                    let _ = app.emit_to(window.label(), MENU_EVENT, id);
                }
                None => {
                    let _ = app.emit(MENU_EVENT, id);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::open_document,
            commands::open_document_window,
            commands::render_page,
            commands::render_thumbnail,
            commands::render_page_region,
            commands::page_info,
            commands::extract_text,
            commands::extract_text_layer,
            commands::close_document,
            text_edit::text_edit_target,
            text_edit::apply_text_edit,
            text_edit::delete_text_box,
            text_edit::edit_history,
            text_edit::undo_text_edit,
            text_edit::redo_text_edit,
            text_edit::mark_document_saved,
        ])
        .setup(|app| {
            // A missing library must not stop the shell from starting; opening a
            // document reports the setup instructions through the command error.
            if let Err(error) = engine::Engine::initialize(Some(app.handle())) {
                eprintln!("[pdfx] PDF engine unavailable: {error}");
            }
            eprintln!("[pdfx] workspace ready");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run PDFX");
}
