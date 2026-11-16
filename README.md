# RustGet

A native Android download manager written in Rust. No WebView, no PWA, no Android Studio, no Gradle,
no accounts, no subscriptions, no ads, no telemetry.

## What it does

| Feature | Details |
|---|---|
| Segmented downloads | Up to 32 parallel connections per file |
| Dynamic re-segmentation | An idle connection splits the biggest in-flight segment in half and takes the back half, so all connections stay busy to the last byte |
| Resume | Segment table saved to disk; survives app kills and reboots; ETag / Last-Modified / size are re-checked so a changed file restarts cleanly |
| Mirrors | `url1 \| url2` – extra URLs are used automatically as fall-backs (and rotated on failure) |
| Retries | Exponential back-off per connection, progress-aware, configurable |
| Speed limits | Global and per-download (adjustable while running) |
| Queue | N downloads at once, "Start now", pause/resume all, scheduling ("start in N minutes") |
| Link grabber | Scan any web page for links, filter (`.zip .pdf`), add the ticked ones in one go |
| Batch + sequences | Paste many links; `img[001-120].jpg` expands to a numbered batch |
| Checksums | Paste an MD5 / SHA-1 / SHA-256; verified after download |
| Auth / headers | Referer, Cookie, HTTP Basic, arbitrary headers per download |
| Sorting | Finished files go to Video / Music / Images / Documents / Archives / Programs (optional) |
| Wi-Fi only | Optional: wait for an unmetered network |
| Clipboard + Share | Offers to download links you copy; "Share → RustGet" from any app |
| Safe finishing | Writes to `.rustget/<id>.part`, moves into place on completion, never overwrites (adds `(1)`) |

Not included (yet): torrents/magnets, FTP, HLS/DASH streams, and a foreground-service notification.

## Build the APK (no Android Studio)

### Option A – from a phone, in the cloud
1. Put this folder in a GitHub repository.
2. **Actions → Build APK → Run workflow.**
3. Download the `RustGet-apk` artifact, unzip, install `RustGet.apk`
   (allow "install unknown apps" for your browser/file manager).

For updates to install over older builds, add secrets `KEYSTORE_B64` (base64 of your keystore) and
`KS_PASS`; otherwise each run signs with a fresh key and you must uninstall first.

### Option B – on a computer
```sh
# one-time: Rust, JDK, Android command-line tools (platform-tools, build-tools, platforms;android-34, ndk)
export ANDROID_HOME=~/Android/Sdk
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version>
./build.sh                       # produces RustGet.apk (arm64)
adb install RustGet.apk          # or copy the file to the phone
```
`build.sh` calls `cargo-ndk`, `aapt2`, `zipalign` and `apksigner` directly.

### Desktop test build
`cargo run --release --bin rustget-desktop` runs the same engine and UI in a window.

## Storage

* Default folder is the app's own external folder (always writable).
* For the public `Download/RustGet` folder (Android 11+): **Settings → Public Downloads**; Android opens
  the "All files access" page, enable it, return, tap again. On Android 7–10 the app folder is used.

## Layout

```
src/logic.rs      pure logic (segment planning, pacing, filenames, parsing, formatting) – unit tested
src/net.rs        HTTP probing / page fetching (ureq + rustls)
src/engine.rs     scheduler, workers, resume, persistence
src/platform.rs   JNI bridge (clipboard, intents, permissions, wake lock, metered check)
src/ui.rs         egui touch UI
src/lib.rs        android_main entry point
```

Run the logic tests anywhere with no dependencies:
`rustc --edition 2021 --test src/logic.rs -o /tmp/t && /tmp/t`

## Known limitations

* Android's soft keyboard with a pure `NativeActivity` is the roughest edge. Paste, Share, the clipboard
  banner and the link grabber avoid typing; a "Keyboard" button is provided, and hardware keyboards work.
* No foreground service, so Android may stop downloads some time after the app is backgrounded
  (a wake lock is held while downloading). Re-opening resumes automatically from where it stopped.
* A "Share" while the app is already open is not re-read; use the clipboard banner in that case.
