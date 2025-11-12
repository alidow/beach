use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string();
    let git_sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .filter(|sha| !sha.is_empty())
        .unwrap_or_else(|| "nogit".to_string());
    let build_id = format!("{timestamp}-{git_sha}");
    println!("cargo:rustc-env=BEACH_BUILD_ID={}", build_id);
}
