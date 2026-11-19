//! `HERDR_NVIM_REV`: the git commit this binary was built from, which the
//! companion handshake (`src/app/companion.rs`) compares. A build whose
//! compiled-in files (`CONTENT`) differ from that commit is
//! `<commit>-dirty-<build time>`, so it matches only itself; outside a git
//! checkout (a crate tarball) it is `unknown`, which matches nothing.

use std::{
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

/// Everything the binary is built from (`include_str!` reaches into
/// herdr-app, lua and herdr-plugin.toml): a change to any of them builds
/// again and stamps the rev again.
const CONTENT: [&str; 8] = [
    "src",
    "lua",
    "plugin",
    "herdr-app",
    "herdr-plugin.toml",
    "build.rs",
    "Cargo.toml",
    "Cargo.lock",
];

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Where the commit is recorded: HEAD, the branch's ref (loose or packed),
/// its log and the index. Only the files that exist: cargo builds again
/// every time for a missing one, and a ref that becomes loose or packed
/// changes one that does.
fn watch_git() {
    let branch = git(&["symbolic-ref", "-q", "HEAD"]);
    let names = ["HEAD", "logs/HEAD", "index", "packed-refs"]
        .into_iter()
        .map(str::to_owned)
        .chain(branch);
    for name in names {
        if let Some(path) = git(&["rev-parse", "--git-path", &name]) {
            if Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            }
        }
    }
}

fn main() {
    for path in CONTENT {
        println!("cargo:rerun-if-changed={path}");
    }
    let rev = match git(&["rev-parse", "HEAD"]) {
        None => "unknown".to_owned(),
        Some(commit) => {
            watch_git();
            let mut status = vec![
                "--no-optional-locks",
                "status",
                "--porcelain",
                "--untracked-files=normal",
                "--",
            ];
            status.extend(CONTENT);
            if git(&status).is_none_or(|changes| !changes.is_empty()) {
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
