// Top-level build file. Rust .so is built separately via cargo-ndk (see README)
// and copied into app/src/main/jniLibs/<abi>/ before `assembleDebug`.
plugins {
    id("com.android.application") version "8.5.2" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
}
