jni := "platform/android/app/src/main/jniLibs"
abis := "-t arm64-v8a -t armeabi-v7a -t x86 -t x86_64"
app_id := `sed -n 's/.*applicationId = "\(.*\)"/\1/p' platform/android/app/build.gradle.kts`

# Debug build for one ABI (default: emulator), install and launch on a device
# of that ABI. e.g. `just android arm64-v8a`; set ANDROID_SERIAL to pick one yourself
android abi="x86_64":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "${ANDROID_SERIAL:-}" ]; then
      # primary ABI first: x86_64 images also list arm64-v8a through translation
      for prop in ro.product.cpu.abi ro.product.cpu.abilist; do
        for serial in $(adb devices | awk 'NR > 1 && $2 == "device" { print $1 }'); do
          if adb -s "$serial" shell getprop "$prop" | tr -d '\r' | tr ',' '\n' | grep -qx "{{abi}}"; then
            export ANDROID_SERIAL="$serial"
            break 2
          fi
        done
      done
      if [ -z "${ANDROID_SERIAL:-}" ]; then
        echo "no device for {{abi}}; connected: $(adb devices | awk 'NR > 1 && NF { print $1 }' | xargs)" >&2
        exit 1
      fi
    fi
    echo "using $ANDROID_SERIAL"
    rm -rf {{jni}}
    cargo ndk -t {{abi}} -P 30 -o {{jni}} build -p aurora_mobile --lib
    # cargo-ndk also copies dependencies built as cdylib (gpui_mobile, iroh);
    # libaurora.so links them statically
    find {{jni}} -name '*.so' ! -name libaurora.so -delete
    (cd platform/android && ./gradlew assembleDebug)
    # not running during the update, so Android 17 does not relaunch it on top
    # of our start (a second activity in one process panics in gpui)
    adb shell am force-stop {{app_id}}
    # streamed install: gradle's installDebug copies the (large) APK to
    # /data/local/tmp first, which newer Android refuses
    adb install -r platform/android/app/build/outputs/apk/debug/app-debug.apk
    adb shell am start -n {{app_id}}/dev.gpui.mobile.GpuiActivity

# Release APK + AAB for all ABIs, signed when platform/android/keystore.properties exists
android-release:
    rm -rf {{jni}}
    cargo ndk {{abis}} -P 30 -o {{jni}} build -p aurora_mobile --lib --release
    # cargo-ndk also copies dependencies built as cdylib (gpui_mobile, iroh);
    # libaurora.so links them statically
    find {{jni}} -name '*.so' ! -name libaurora.so -delete
    cd platform/android && ./gradlew assembleRelease bundleRelease

# macOS only: generate the Xcode project (its pre-build phase runs cargo)
ios:
    #!/usr/bin/env bash
    set -euo pipefail
    cd platform/ios && xcodegen generate --spec project.yml
    scheme=$(sed -n 's/^name: //p' project.yml)
    xcodebuild -project "$scheme.xcodeproj" -scheme "$scheme" -sdk iphonesimulator -configuration Debug -derivedDataPath build build

# Run on desktop (Linux / macOS / Windows), also handy for quick UI iteration
desktop:
    cargo run -p aurora_gui

# Run the daemon
daemon:
    cargo run -p aurora_daemon

# Replace the app icon everywhere from one square PNG (1024x1024 recommended). e.g. `just icon logo.png`
icon src:
    #!/usr/bin/env bash
    set -euo pipefail
    res=platform/android/app/src/main/res
    for d in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:144 xxxhdpi:192; do
        size=${d#*:}
        magick "{{src}}" -resize "${size}x${size}" "$res/mipmap-${d%:*}/ic_launcher.png"
        magick "{{src}}" -resize "${size}x${size}" \
            \( +clone -alpha extract -fill black -colorize 100 -fill white -draw "circle $((size/2)),$((size/2)) $((size/2)),0" \) \
            -alpha off -compose CopyOpacity -composite "$res/mipmap-${d%:*}/ic_launcher_round.png"
    done
    # App Store rejects icons with an alpha channel
    magick "{{src}}" -resize 1024x1024 -background white -alpha remove -alpha off platform/ios/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png
    # desktop packaging (absent in mobile-only projects)
    if [ -d platform/packaging/icons ]; then
        magick "{{src}}" -resize 32x32 platform/packaging/icons/32x32.png
        magick "{{src}}" -resize 128x128 platform/packaging/icons/128x128.png
        magick "{{src}}" -resize 256x256 platform/packaging/icons/128x128@2x.png
        magick "{{src}}" -define icon:auto-resize=256,128,64,48,32,16 platform/packaging/icons/icon.ico
    fi
    echo "icon updated from {{src}}"
