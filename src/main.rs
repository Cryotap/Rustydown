//! Desktop build of RustGet (same engine and UI as the Android app), for testing on a computer.
//! Android uses `android_main` in lib.rs instead.

#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result<()> {
    rustget::run_desktop()
}

#[cfg(target_os = "android")]
fn main() {}
