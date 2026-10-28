use pdfx_lib::commands::{app_info, AppInfo};

#[test]
fn app_info_returns_the_pdfx_product_identity() {
    let info: AppInfo = app_info();

    assert_eq!(info.name, "PDFX");
    assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
}

#[test]
fn app_info_is_serializable_across_the_tauri_command_boundary() {
    let info = app_info();
    let json = serde_json::to_value(&info).expect("AppInfo should serialize");

    assert_eq!(json["name"], "PDFX");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
}
