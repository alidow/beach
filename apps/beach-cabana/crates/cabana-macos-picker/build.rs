use std::env;

fn main() {
    // Rebuild if bridge sources change.
    println!("cargo:rerun-if-changed=bridge/CabanaPickerBridge.m");
    println!("cargo:rerun-if-changed=bridge/CabanaPickerBridge.h");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let has_native_feature = env::var("CARGO_FEATURE_NATIVE").is_ok();

    if target_os != "macos" || !has_native_feature {
        // Nothing to compile on non-mac targets or when the native feature is disabled.
        return;
    }

    // Require macOS 14 for SCContentSharingPicker APIs.
    env::set_var("MACOSX_DEPLOYMENT_TARGET", "14.0");
    println!("cargo:rustc-env=MACOSX_DEPLOYMENT_TARGET=14.0");
    println!("cargo:rustc-link-arg=-mmacosx-version-min=14.0");

    let mut build = cc::Build::new();
    build.file("bridge/CabanaPickerBridge.m");
    build.flag("-fobjc-arc");
    build.flag("-fmodules");
    build.compile("cabana_picker_bridge");

    println!("cargo:rustc-link-lib=framework=ScreenCaptureKit");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=Foundation");
}
