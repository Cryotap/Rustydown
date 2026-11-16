//! RustGet: a native (non-WebView, non-PWA) Android download manager written in Rust.

mod engine;
mod logic;
mod net;
mod platform;
mod ui;

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: winit::platform::android::activity::AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );

    let data_dir = app
        .internal_data_path()
        .unwrap_or_else(|| std::path::PathBuf::from("/data/local/tmp/rustget"));
    // App-private external folder: always writable. The public Downloads folder needs
    // "All files access", which the user can grant from Settings inside the app.
    let app_dir = app
        .external_data_path()
        .unwrap_or_else(|| data_dir.clone())
        .join("Download")
        .to_string_lossy()
        .into_owned();
    let public_dir = "/storage/emulated/0/Download/RustGet".to_string();
    let default_dir = if platform::has_all_files_access() {
        public_dir
    } else {
        app_dir.clone()
    };

    let options = eframe::NativeOptions {
        android_app: Some(app),
        ..Default::default()
    };
    eframe::run_native(
        "RustGet",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, data_dir, default_dir, app_dir)))),
    )
    .expect("failed to start UI");
}

/// Desktop entry point, used by `src/main.rs` for quick testing on a computer.
#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> eframe::Result<()> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let data_dir = std::path::PathBuf::from(&home).join(".local/share/rustget");
    let dl_dir = format!("{home}/Downloads/RustGet");
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([420.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "RustGet",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(cc, data_dir, dl_dir.clone(), dl_dir)))),
    )
}
