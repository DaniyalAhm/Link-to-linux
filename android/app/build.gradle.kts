plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "org.linktolinux.app"
    compileSdk = 34
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "org.linktolinux.app"
        minSdk = 26
        targetSdk = 34
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        versionCode = 2
        versionName = "0.1.0-pairing-codes"
    }

    buildTypes {
        getByName("debug") {
            applicationIdSuffix = ".debug"
        }
        getByName("release") {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    // Rust cdylibs built by cargo-ndk go here:
    //   app/src/main/jniLibs/arm64-v8a/liblink_android_svc.so
    //   app/src/main/jniLibs/armeabi-v7a/...
    //   app/src/main/jniLibs/x86_64/... (emulator)
    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }
}

// Always rebuild both supported ABIs when native code changes. A Kotlin-only
// APK build must never silently package an older physical-phone protocol.
val buildRust by tasks.registering(Exec::class) {
    workingDir(rootProject.projectDir.parentFile)
    inputs.files(fileTree("../../crates/link-core") { include("src/**", "Cargo.toml") })
    inputs.files(fileTree("../../crates/link-android-svc") { include("src/**", "Cargo.toml") })
    inputs.files("../../Cargo.toml", "../../Cargo.lock")
    outputs.files("src/main/jniLibs/arm64-v8a/liblink_android_svc.so", "src/main/jniLibs/x86_64/liblink_android_svc.so")
    environment("ANDROID_HOME", android.sdkDirectory.absolutePath)
    environment("ANDROID_NDK_HOME", "${android.sdkDirectory}/ndk/${android.ndkVersion}")
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64", "-o", "android/app/src/main/jniLibs",
        "build", "-p", "link-android-svc", "--release")
}
tasks.named("preBuild") { dependsOn(buildRust) }

dependencies {
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("junit:junit:4.13.2")
    // Only the AOSP-derived MMS PDU codec is used; SmsManager owns transport.
    implementation("com.klinkerapps:android-smsmms:5.2.6") { isTransitive = false }
    implementation("com.klinkerapps:logger:1.0.3")
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("com.google.android.material:material:1.12.0")
}
