pub mod commands;
mod menu;

pub use commands::{app_info, AppInfo};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .menu(menu::build_menu)
        .on_menu_event(|_app, event| {
            // Placeholder wiring: later tickets route these ids to editor commands.
            eprintln!("[pdfx] menu event: {}", event.id().as_ref());
        })
        .invoke_handler(tauri::generate_handler![commands::app_info])
        .setup(|_app| {
            eprintln!("[pdfx] workspace ready");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run PDFX");
}
