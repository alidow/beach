use chrono::Utc;

fn main() {
    // Generate build timestamp
    let timestamp = Utc::now().format("%Y%m%d%H%M%S").to_string();
    println!("cargo:rustc-env=BUILD_TIMESTAMP={}", timestamp);

    // Rerun if build.rs changes
    println!("cargo:rerun-if-changed=build.rs");

    // Also rerun if any source files change to ensure fresh timestamps
    println!("cargo:rerun-if-changed=src/");
}
