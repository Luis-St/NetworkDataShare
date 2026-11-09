//! Builds the Svelte frontend in `web/` into `$OUT_DIR/web-dist`, which is embedded into the binary.
//!
//! Building into `OUT_DIR` instead of `web/dist` means a deleted or stale `web/dist` can never end
//! up in the binary. Set `NDS_SKIP_WEB_BUILD=1` to skip npm and embed an existing `web/dist` instead.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

const INPUTS: &[&str] = &[
    "web/src",
    "web/public",
    "web/index.html",
    "web/package.json",
    "web/package-lock.json",
    "web/vite.config.ts",
    "web/svelte.config.js",
    "web/tsconfig.json",
];

fn main() {
    println!("cargo:rerun-if-env-changed=NDS_SKIP_WEB_BUILD");
    for path in INPUTS {
        println!("cargo:rerun-if-changed={path}");
    }

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("web-dist");
    if std::env::var_os("NDS_SKIP_WEB_BUILD").is_some() {
        let _ = fs::remove_dir_all(&out);
        fs::create_dir_all(&out).unwrap();
        copy_dir(Path::new("web/dist"), &out);
        return;
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    if dependencies_outdated() {
        run(Command::new(npm).args(["ci", "--no-audit", "--no-fund"]).current_dir("web"), "npm ci");
    }
    let out_arg = out.to_str().expect("OUT_DIR is not valid UTF-8");
    run(
        Command::new(npm).args(["run", "build", "--", "--outDir", out_arg, "--emptyOutDir"]).current_dir("web"),
        "npm run build",
    );
}

/// `npm ci` is needed when `node_modules` is missing or older than the lockfile.
fn dependencies_outdated() -> bool {
    let modified = |p: &str| fs::metadata(p).and_then(|m| m.modified()).ok();
    let installed = modified("web/node_modules/.package-lock.json");
    let lock = modified("web/package-lock.json").unwrap_or(SystemTime::UNIX_EPOCH);
    installed.is_none_or(|i| i < lock)
}

fn copy_dir(from: &Path, to: &Path) {
    let Ok(entries) = fs::read_dir(from) else { return };
    for entry in entries.flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("failed to run `{what}` (is Node.js installed?): {e}"));
    if !status.success() {
        panic!("`{what}` failed with {status}");
    }
}
