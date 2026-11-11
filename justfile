jni := "platform/android/app/src/main/jniLibs"
abis := "-t arm64-v8a -t armeabi-v7a -t x86 -t x86_64"
app_id := `sed -n 's/.*applicationId = "\(.*\)"/\1/p' platform/android/app/build.gradle.kts`

# Debug build for one ABI (default: emulator), install and launch. e.g. `just android arm64-v8a`
android abi="x86_64":
    rm -rf {{jni}}
    cargo ndk -t {{abi}} -P 30 -o {{jni}} build -p aurora_mobile --lib
    rm -f {{jni}}/*/libgpui_mobile-*.so
    cd platform/android && ./gradlew installDebug
    adb shell am start -n {{app_id}}/dev.gpui.mobile.GpuiActivity

# Release APK + AAB for all ABIs, signed when platform/android/keystore.properties exists
android-release:
    rm -rf {{jni}}
    cargo ndk {{abis}} -P 30 -o {{jni}} build -p aurora_mobile --lib --release
    rm -f {{jni}}/*/libgpui_mobile-*.so
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
