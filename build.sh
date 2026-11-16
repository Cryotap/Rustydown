#!/usr/bin/env bash
# Builds RustGet.apk from the command line. No Android Studio, no Gradle.
#
# Needs: Rust (rustup), the Android SDK command-line tools (platform + build-tools),
#        the Android NDK, a JDK (for keytool), `zip`, and cargo-ndk.
#
#   export ANDROID_HOME=/path/to/sdk
#   export ANDROID_NDK_HOME=/path/to/ndk
#   ./build.sh                 # arm64 phones (default)
#   ABIS="arm64-v8a armeabi-v7a x86_64" ./build.sh
set -euo pipefail
cd "$(dirname "$0")"

: "${ANDROID_HOME:?Set ANDROID_HOME to your Android SDK folder}"
: "${ANDROID_NDK_HOME:?Set ANDROID_NDK_HOME to your Android NDK folder}"
API="${API:-34}"
ABIS="${ABIS:-arm64-v8a}"
KEYSTORE="${KEYSTORE:-rustget.keystore}"
KS_PASS="${KS_PASS:-android}"

BT="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
JAR="$ANDROID_HOME/platforms/android-$API/android.jar"
[ -f "$JAR" ] || { echo "Missing $JAR (install platform android-$API)"; exit 1; }

# Rust targets + cargo-ndk
targets=(); ndk_args=()
for abi in $ABIS; do
  case "$abi" in
    arm64-v8a)   targets+=(aarch64-linux-android) ;;
    armeabi-v7a) targets+=(armv7-linux-androideabi) ;;
    x86_64)      targets+=(x86_64-linux-android) ;;
    x86)         targets+=(i686-linux-android) ;;
    *) echo "Unknown ABI $abi"; exit 1 ;;
  esac
  ndk_args+=(-t "$abi")
done
rustup target add "${targets[@]}"
command -v cargo-ndk >/dev/null || cargo install cargo-ndk --locked

echo "==> Compiling Rust"
rm -rf build && mkdir -p build/stage
cargo ndk "${ndk_args[@]}" --platform 24 -o build/stage/lib build --release --lib

echo "==> Packaging"
"$BT/aapt2" link -o build/base.apk \
  --manifest AndroidManifest.xml -I "$JAR" \
  --min-sdk-version 24 --target-sdk-version "$API" \
  --version-code 1 --version-name 0.1.0
# native libraries must be stored uncompressed so Android can map them straight from the APK
(cd build/stage && zip -q -r -0 ../base.apk lib)

if ! "$BT/zipalign" -f -P 16 4 build/base.apk build/aligned.apk 2>/dev/null; then
  "$BT/zipalign" -f -p 4 build/base.apk build/aligned.apk
fi

if [ ! -f "$KEYSTORE" ]; then
  echo "==> Creating signing key $KEYSTORE (keep it: updates must be signed with the same key)"
  keytool -genkeypair -keystore "$KEYSTORE" -alias rustget -keyalg RSA -keysize 2048 \
    -validity 10000 -storepass "$KS_PASS" -keypass "$KS_PASS" -dname "CN=RustGet"
fi
"$BT/apksigner" sign --ks "$KEYSTORE" --ks-key-alias rustget \
  --ks-pass "pass:$KS_PASS" --key-pass "pass:$KS_PASS" \
  --out RustGet.apk build/aligned.apk
"$BT/apksigner" verify RustGet.apk
echo "==> Done: $(pwd)/RustGet.apk"
