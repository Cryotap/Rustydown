# RustGet

Android download manager: **Rust engine + Kotlin UI**, built the same way as RustBrowser
(GitHub Actions → cargo-ndk → Gradle). No Android Studio, no accounts, no subscriptions, no ads.

## Build
Push to GitHub, then **Actions → Build APK → Run workflow**. Download the `RustGet-apk` artifact and install it.
The keystore is committed (`android/rustget-release.jks`), so every build is signed the same way and
installs over the previous one. To use your own, set secrets `KEYSTORE_BASE64`, `KEYSTORE_PASSWORD`,
`KEY_ALIAS`, `KEY_PASSWORD`.

## Features
- Up to 32 connections per file, with dynamic re-segmentation (an idle connection splits the busiest segment)
- Resume across app kills and reboots; ETag / Last-Modified / size checks
- Mirrors (`url1 | url2`), exponential-back-off retries
- Global and per-download speed limits, queue, scheduled starts, optional "wait for Wi-Fi"
- Link grabber (scan a page, filter, add many), batch paste, `img[001-120].jpg` sequences
- MD5 / SHA-1 / SHA-256 verification, Referer / Cookie / Basic auth / custom headers
- Foreground service with progress notification and "Pause all"
- Clipboard offer, Share-to-RustGet, sort finished files by type

## Layout
```
src/logic.rs    pure logic, unit tested (cargo test --lib)
src/net.rs      HTTP probing (ureq + rustls)
src/engine.rs   scheduler, workers, resume, persistence
src/lib.rs      JNI exports (Java_com_rustget_app_Native_*)
android/        Gradle project: Native.kt, MainActivity.kt, DownloadService.kt
```

## Not included yet
Torrents/magnets, FTP, HLS/DASH, opening finished files from the list (needs a FileProvider).
