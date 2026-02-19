use std::path::Path;
use std::process::Command;

fn main() {
    // Only build the frontend when the embed-ui feature is active.
    if std::env::var("CARGO_FEATURE_EMBED_UI").is_err() {
        return;
    }

    let ui_dir = Path::new("../../apps/forwarder-ui");
    assert!(
        ui_dir.join("package.json").exists(),
        "Cannot find apps/forwarder-ui/package.json — run from the workspace root"
    );

    // Rerun when frontend source changes.
    println!("cargo:rerun-if-changed=../../apps/forwarder-ui/package.json");
    println!("cargo:rerun-if-changed=../../apps/forwarder-ui/package-lock.json");
    println!("cargo:rerun-if-changed=../../apps/forwarder-ui/tsconfig.json");
    println!("cargo:rerun-if-changed=../../apps/forwarder-ui/src");
    println!("cargo:rerun-if-changed=../../apps/forwarder-ui/static");
    println!("cargo:rerun-if-changed=../../apps/forwarder-ui/svelte.config.js");
    println!("cargo:rerun-if-changed=../../apps/forwarder-ui/vite.config.ts");
    println!("cargo:rerun-if-changed=../../apps/shared-ui/src");

    // npm ci — install dependencies.
    let status = Command::new("npm")
        .args(["ci"])
        .current_dir(ui_dir)
        .status()
        .expect("failed to run npm ci — is Node.js installed?");
    assert!(status.success(), "npm ci failed");

    // npm run build — produce static assets in build/.
    let status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(ui_dir)
        .status()
        .expect("failed to run npm run build");
    assert!(status.success(), "npm run build failed");
}
