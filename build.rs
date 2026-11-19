//! `HERDR_NVIM_REV`: the git commit this binary was built from, which the
//! companion handshake (`src/app/companion.rs`) compares. A build with local
//! changes is `<commit>-dirty-<build time>`, so it matches only itself;
//! outside a git checkout (a crate tarball) it is `unknown`, which matches
//! nothing.

use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn main() {
    for path in [
        "src",
        "lua",
        "plugin",
        "herdr-app",
        "Cargo.toml",
        "Cargo.lock",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let rev = match git(&["rev-parse", "HEAD"]) {
        None => "unknown".to_owned(),
        Some(commit) => {
            for name in ["HEAD", "index", "packed-refs"] {
                if let Some(path) = git(&["rev-parse", "--git-path", name]) {
                    println!("cargo:rerun-if-changed={path}");
                }
            }
            if let Some(path) = git(&["symbolic-ref", "-q", "HEAD"])
                .and_then(|branch| git(&["rev-parse", "--git-path", &branch]))
            {
                println!("cargo:rerun-if-changed={path}");
            }
            let changed = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_none_or(|status| !status.is_empty());
            if changed {
                let at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                format!("{commit}-dirty-{at}")
            } else {
                commit
            }
        }
    };
    println!("cargo:rustc-env=HERDR_NVIM_REV={rev}");
}
