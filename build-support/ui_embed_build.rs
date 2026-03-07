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
    if std::env::var("CARGO_FEATURE_EMBED_UI").is_err() {
        return;
    }

    let ui_dir = Path::new(UI_APP_DIR);
    assert!(
        ui_dir.join("package.json").exists(),
        "Cannot find {UI_APP_PATH}/package.json — run from the workspace root"
    );

    println!("cargo:rerun-if-changed={UI_APP_DIR}/package.json");
    println!("cargo:rerun-if-changed=../../package-lock.json");
    println!("cargo:rerun-if-changed={UI_APP_DIR}/tsconfig.json");
    println!("cargo:rerun-if-changed={UI_APP_DIR}/src");
    println!("cargo:rerun-if-changed={UI_APP_DIR}/static");
    println!("cargo:rerun-if-changed={UI_APP_DIR}/svelte.config.js");
    println!("cargo:rerun-if-changed={UI_APP_DIR}/vite.config.ts");
    println!("cargo:rerun-if-changed=../../apps/shared-ui/src");

    // When cross-compiling, npm is not available inside the container.
    // Skip the build only if cross-compiling AND the output already exists.
    let is_cross_compiling = std::env::var("CROSS_COMPILE").is_ok()
        || std::env::var("CARGO_CFG_TARGET_OS")
            .map(|t| t != std::env::consts::OS)
            .unwrap_or(false)
        || std::env::var("CARGO_CFG_TARGET_ARCH")
            .map(|t| t != std::env::consts::ARCH)
            .unwrap_or(false);
    let build_output = ui_dir.join("build");
    if is_cross_compiling && build_output.join("index.html").exists() {
        println!("cargo:warning=UI already built at {}, skipping npm build (cross-compile)", build_output.display());
        return;
    }

    let workspace_root = Path::new("../../");
    assert!(
        workspace_root.join("node_modules").exists(),
        "node_modules not found — run `npm install` from the workspace root first"
    );

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
