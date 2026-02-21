use std::path::Path;
use std::process::Command;

fn npm_program_for(is_windows: bool) -> &'static str {
    if is_windows {
        "npm.cmd"
    } else {
        "npm"
    }
}

fn npm_program() -> &'static str {
    npm_program_for(cfg!(windows))
}

fn main() {
    // Only build the frontend when the embed-ui feature is active.
    if std::env::var("CARGO_FEATURE_EMBED_UI").is_err() {
        return;
    }

    let ui_dir = Path::new("../../apps/receiver-ui");
    assert!(
        ui_dir.join("package.json").exists(),
        "Cannot find apps/receiver-ui/package.json — run from the workspace root"
    );

    // Rerun when frontend source changes.
    println!("cargo:rerun-if-changed=../../apps/receiver-ui/package.json");
    println!("cargo:rerun-if-changed=../../package-lock.json");
    println!("cargo:rerun-if-changed=../../apps/receiver-ui/tsconfig.json");
    println!("cargo:rerun-if-changed=../../apps/receiver-ui/src");
    println!("cargo:rerun-if-changed=../../apps/receiver-ui/static");
    println!("cargo:rerun-if-changed=../../apps/receiver-ui/svelte.config.js");
    println!("cargo:rerun-if-changed=../../apps/receiver-ui/vite.config.ts");
    println!("cargo:rerun-if-changed=../../apps/shared-ui/src");

    // Verify that dependencies are installed (dev.py / CI handles npm install
    // before cargo build; we just need to run the build here).
    let workspace_root = Path::new("../../");
    assert!(
        workspace_root.join("node_modules").exists(),
        "node_modules not found — run `npm install` from the workspace root first"
    );

    // npm run build — produce static assets in build/.
    // On Windows, npm is exposed as npm.cmd (not npm.exe), so pick the
    // platform-specific program name explicitly.
    let status = Command::new(npm_program())
        .args(["run", "build"])
        .current_dir(ui_dir)
        .status()
        .expect("failed to run npm run build");
    assert!(status.success(), "npm run build failed");
}

#[cfg(test)]
mod tests {
    use super::{npm_program, npm_program_for};

    #[test]
    fn npm_program_for_windows_uses_cmd_wrapper() {
        assert_eq!(npm_program_for(true), "npm.cmd");
    }

    #[test]
    fn npm_program_for_non_windows_uses_plain_npm() {
        assert_eq!(npm_program_for(false), "npm");
    }

    #[test]
    fn npm_program_matches_current_target() {
        let expected = if cfg!(windows) { "npm.cmd" } else { "npm" };
        assert_eq!(npm_program(), expected);
    }
}
