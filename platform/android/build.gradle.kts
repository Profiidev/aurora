// Root build.gradle.kts for the GPUI Mobile Android Example project.
//
// This is a minimal Gradle project that packages the Rust native library
// (compiled separately via cargo-ndk) into an APK using NativeActivity.
//
// Build steps:
//   1. Compile the Rust library:
//      cargo ndk -t arm64-v8a -o app/src/main/jniLibs build --example android_app --release
//
//   2. Build the APK:
//      ./gradlew assembleDebug
//
//   3. Install on device/emulator:
//      adb install app/build/outputs/apk/debug/app-debug.apk

buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.1.0")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.22")
    }
}

tasks.register("clean", Delete::class) {
    delete(rootProject.layout.buildDirectory)
}
