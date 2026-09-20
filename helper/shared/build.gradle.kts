plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.jevpilot.helper"
    compileSdk = 36

    defaultConfig {
        minSdk = 24
    }

    sourceSets {
        named("main") { java.srcDirs("src/main/kotlin") }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
        allWarningsAsErrors.set(true)
    }
}

dependencies {
    // The Android SDK alone: android.accessibilityservice,
    // android.view.accessibility and org.json. No third-party code runs in a
    // process that can read every screen on the device.
}
