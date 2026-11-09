//! Builds the Svelte frontend in `web/` so it can be embedded into the binary.
//!
//! Set `NDS_SKIP_WEB_BUILD=1` to skip this step (e.g. when `web/dist` is built separately).

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=NDS_SKIP_WEB_BUILD");
    for path in ["web/src", "web/index.html", "web/package.json", "web/vite.config.ts", "web/public"] {
        println!("cargo:rerun-if-changed={path}");
    }

    let dist = Path::new("web/dist");
    if std::env::var_os("NDS_SKIP_WEB_BUILD").is_some() {
        std::fs::create_dir_all(dist).expect("failed to create web/dist");
        return;
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    if !Path::new("web/node_modules").exists() {
        run(Command::new(npm).args(["install", "--no-audit", "--no-fund"]).current_dir("web"), "npm install");
    }
    run(Command::new(npm).args(["run", "build"]).current_dir("web"), "npm run build");
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to run `{what}` (is Node.js installed?): {e}"));
    if !status.success() {
        panic!("`{what}` failed with {status}");
    }
}
