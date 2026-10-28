pub mod annotations;
mod atomic_write;
pub mod commands;
pub mod engine;
pub mod file_association;
pub mod flatten_export;
mod fonts;
pub mod form;
pub mod form_create;
pub mod image_edit;
pub mod line_edit;
pub mod logging;
mod menu;
pub mod page_ops;
pub mod pdfium_adapter;
pub mod print_export;
pub mod recovery;
pub mod save;
pub mod settings;
mod stamp_glyphs;
pub mod stamps;
mod target_path;
pub mod text_edit;
pub mod updater;

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
            page_ops::rotate_page,
            page_ops::delete_page,
            page_ops::move_page,
            page_ops::insert_blank_page,
            page_ops::copy_page,
            text_edit::text_edit_target,
            text_edit::apply_text_edit,
            text_edit::delete_text_box,
            text_edit::text_box_fonts,
            text_edit::create_text_box,
            text_edit::text_boxes,
            text_edit::apply_text_box_style,
            text_edit::transform_text_box,
            text_edit::edit_history,
            text_edit::undo_text_edit,
            text_edit::redo_text_edit,
            text_edit::mark_document_saved,
            form::form_fields,
            form::missing_required_fields,
            form::fill_form_field,
            form_create::create_form_field,
            form_create::update_form_field,
            form_create::delete_form_field,
            form_create::create_form_text_field,
            form_create::update_form_text_field,
            form_create::delete_form_text_field,
            image_edit::insert_image,
            image_edit::page_images,
            image_edit::transform_image,
            image_edit::delete_image,
            line_edit::create_line_object,
            line_edit::page_line_objects,
            line_edit::transform_line_object,
            line_edit::update_line_style,
            line_edit::delete_line_object,
            annotations::create_text_markup_annotation,
            annotations::create_note_annotation,
            annotations::create_ink_annotation,
            annotations::create_free_text_annotation,
            annotations::create_shape_annotation,
            annotations::create_stamp_annotation,
            annotations::update_shape_geometry,
            annotations::update_shape_style,
            annotations::update_free_text,
            annotations::transform_free_text_annotation,
            annotations::transform_stamp_annotation,
            annotations::list_annotations,
            annotations::update_annotation_style,
            annotations::update_note_text,
            annotations::transform_note_annotation,
            annotations::delete_annotation,
            annotations::erase_ink_stroke,
            print_export::export_page,
            print_export::print_document,
            flatten_export::export_flattened_copy,
            save::save_document,
            recovery::recovery_snapshot,
            commands::recovery_candidates,
            commands::restore_recovery,
            commands::discard_recovery,
            commands::list_recent_files,
            commands::forget_recent_file,
            commands::get_settings,
            commands::update_settings,
            commands::export_diagnostics,
            commands::check_for_updates,
            commands::open_download_page,
            menu::set_menu_state,
        ])
        .setup(|app| {
            // Logging is local-only and must never prevent the shell from
            // starting; a failure falls back to stderr.
            match app.path().app_log_dir() {
                Ok(directory) => {
                    if let Err(error) = logging::initialize(directory) {
                        eprintln!("[pdfx] could not initialize local logging: {error}");
                    }
                }
                Err(error) => {
                    eprintln!("[pdfx] could not resolve the log directory: {error}");
                }
            }
            logging::event(
                "app.start",
                &format!("version={}", env!("CARGO_PKG_VERSION")),
            );

            // Recovery Files and the recent-files list live in the application
            // data directory, separate from user documents and settings. A
            // failure here degrades those features but never blocks the shell.
            match app.path().app_data_dir() {
                Ok(directory) => {
                    if let Err(error) = recovery::initialize(directory) {
                        eprintln!("[pdfx] could not initialize recovery storage: {error}");
                        logging::error("recovery.init.failed", &error.to_string());
                    }
                }
                Err(error) => {
                    eprintln!("[pdfx] could not resolve the application data directory: {error}");
                    logging::error("recovery.init.failed", &error.to_string());
                }
            }

            // A missing library must not stop the shell from starting; opening a
            // document reports the setup instructions through the command error.
            if let Err(error) = engine::Engine::initialize(Some(app.handle())) {
                eprintln!("[pdfx] PDF engine unavailable: {error}");
                logging::error("engine.unavailable", &error);
            }
            eprintln!("[pdfx] workspace ready");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run PDFX");
}
