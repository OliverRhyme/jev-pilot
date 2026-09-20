plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.jevpilot.helper"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.jevpilot.helper"
        // API 24 is the floor for the accessibility APIs this uses; API 36 is
        // what it is built and tested against.
        minSdk = 24
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    sourceSets {
        named("main") {
            java.srcDirs("src/main/kotlin")
        }
    }

    signingConfigs {
        create("bundled") {
            // A checked-in debug key. The APK is a development tool installed
            // over adb, never distributed, and a stable key is what lets
            // `adb install -r` upgrade in place instead of refusing.
            storeFile = file("../debug.keystore")
            storePassword = "android"
            keyAlias = "androiddebugkey"
            keyPassword = "android"
        }
    }

    buildTypes {
        release {
            // Kotlin's stdlib is most of the APK otherwise. R8 keeps the
            // manifest entry points automatically; nothing here is reached by
            // reflection, so what it removes is genuinely unreachable.
            isMinifyEnabled = true
            isShrinkResources = true
            signingConfig = signingConfigs.getByName("bundled")
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    dependenciesInfo {
        // Keeps the dependency block out of the APK: it is signing metadata
        // this tool has no use for.
        includeInApk = false
        includeInBundle = false
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
        allWarningsAsErrors.set(true)
    }
}

dependencies {
    // The Android SDK alone: android.accessibilityservice, android.view.accessibility
    // and org.json. No third-party code runs inside a process that can read every
    // screen on the device.
}
