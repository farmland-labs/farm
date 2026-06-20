//! Build script to capture git information at compile time

use std::process::Command;

fn main() {
    // Re-run if git state changes
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");

    // Get git short SHA
    let git_sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Get git describe (tag + commits since tag + dirty flag)
    let git_describe = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    
    // Get git commit date (YYYY-MM-DD format, like rustc)
    let git_date = Command::new("git")
        .args(["show", "-s", "--format=%cs", "HEAD"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    // Emit as environment variables for the build
    println!("cargo:rustc-env=FARM_GIT_SHA={}", git_sha);
    println!("cargo:rustc-env=FARM_GIT_DESCRIBE={}", git_describe);
    println!("cargo:rustc-env=FARM_GIT_DATE={}", git_date);
}
