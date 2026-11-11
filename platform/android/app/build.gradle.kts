// App module build.gradle.kts for the GPUI Mobile Android Example.
//
// This module packages the pre-compiled Rust native library into an APK
// that uses Android's NativeActivity to host the GPUI application.
//
// The Rust library must be compiled separately and placed into
// app/src/main/jniLibs/<abi>/ before building the APK.
//
// Quick start: `just android`

import java.util.Properties

plugins {
    id("com.android.application")
}

// Single source of truth for the version: the root Cargo.toml.
val cargoVersion = rootProject.file("../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }.substringAfter('"').substringBefore('"')
val (vMajor, vMinor, vPatch) = cargoVersion.split(".").map { it.toInt() }

// Same format as positron's CI: keyAlias, password, storeFile.
val keystoreProperties = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

android {
    namespace = "io.profidev.aurora"
    compileSdk = 34

    defaultConfig {
        applicationId = "io.profidev.aurora"
        minSdk = 30          // Wear OS 3.0+ and modern Android baseline
        targetSdk = 34
        versionCode = vMajor * 10000 + vMinor * 100 + vPatch
        versionName = cargoVersion

        // Tell NativeActivity which .so to load.
        // This must match the cdylib / example output name.
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86", "x86_64")
        }

        // Forward the library name to the manifest via a placeholder.
        manifestPlaceholders["nativeLibraryName"] = "aurora"
    }

    signingConfigs {
        if (!keystoreProperties.isEmpty) {
            create("release") {
                keyAlias = keystoreProperties["keyAlias"] as String
                keyPassword = keystoreProperties["password"] as String
                storeFile = file(keystoreProperties["storeFile"] as String)
                storePassword = keystoreProperties["password"] as String
            }
        }
    }

    buildTypes {
        release {
            // Without keystore.properties the release APK is left unsigned.
            signingConfig = signingConfigs.findByName("release")
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
        }
        debug {
            isDebuggable = true
            isJniDebuggable = true
        }
    }

    // We do NOT use CMake / ndk-build — the native library is compiled
    // externally via cargo-ndk and placed directly into jniLibs.
    //
    // Disable the built-in native build system so Gradle doesn't look for
    // a CMakeLists.txt or Android.mk.
    externalNativeBuild {
        // Intentionally left empty.
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }

    // Tell Gradle where the pre-built .so files live.
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }

    packaging {
        // Prevent stripping of the Rust library — cargo already strips in
        // release mode and stripping again can break backtraces.
        jniLibs {
            keepDebugSymbols += listOf(
                "*/arm64-v8a/libaurora.so",
                "*/armeabi-v7a/libaurora.so",
                "*/x86_64/libaurora.so",
                "*/x86/libaurora.so"
            )
        }
    }

    // Lint configuration — relaxed for an example project.
    lint {
        abortOnError = false
        checkReleaseBuilds = false
    }
}

dependencies {
    // AndroidX core for NotificationCompat (used by GpuiNotifications)
    implementation("androidx.core:core:1.12.0")
    // AndroidX SplashScreen compat (used by GpuiActivity to hold splash until native init)
    implementation("androidx.core:core-splashscreen:1.0.1")
    // AndroidX Biometric for BiometricPrompt (used by GpuiAuthActivity)
    implementation("androidx.biometric:biometric:1.1.0")
    // AndroidX Media for MediaSessionCompat (used by GpuiMediaSession for system controls)
    implementation("androidx.media:media:1.7.1")

}
