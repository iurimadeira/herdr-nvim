//! A remote tab's tree work, done by the herdr-nvim installed on its machine
//! (the companion): `hc exec --on <machine> -- herdr-nvim <app-command> ...`.
//! Each delegation first asks the companion which build it is
//! (`app-version`) and refuses any but this one's, so a missing or other
//! companion is an explicit `CompanionError`, never a different answer.

use std::{fmt, process::Command, process::Stdio};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{cli, remote};

const REPO: &str = "https://github.com/iurimadeira/herdr-nvim";

/// Seconds for the handshake, then the work: together under the app's
/// command deadline (`main.rs` `SHORT`).
const HANDSHAKE_SECS: u32 = 8;
const WORK_SECS: u32 = 14;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub version: String,
    pub rev: String,
}

impl Identity {
    pub fn this_build() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            rev: env!("HERDR_NVIM_REV").to_owned(),
        }
    }

    fn commit(&self) -> Option<&str> {
        let commit = self.rev.split('-').next()?;
        (commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit())).then_some(commit)
    }
}

/// `herdr-nvim app-version`: this build, as the handshake reads it.
pub fn version_cmd() -> Result<()> {
    println!("{}", serde_json::to_string(&Identity::this_build())?);
    Ok(())
}

/// Pure: whether a companion that reported `found` may serve `this` build:
/// the same version and the same commit, from a build without local changes
/// (`build.rs`: those, and builds outside git, carry a rev no other build
/// has).
pub fn matches(this: &Identity, found: &Identity) -> bool {
    this == found && this.commit().is_some()
}

/// Pure: the command that installs this build on a machine, where its core's
/// PATH finds it (`~/.local/bin`).
pub fn install_command(this: &Identity) -> String {
    let pin = this
        .commit()
        .map_or_else(String::new, |commit| format!(" --rev {commit}"));
    format!("cargo install --locked --root ~/.local --git {REPO}{pin} herdr-nvim")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Problem {
    Missing,
    Mismatch,
}

#[derive(Debug)]
pub struct CompanionError {
    pub machine: String,
    pub problem: Problem,
    pub found: Option<Identity>,
    pub expected: Identity,
}

impl fmt::Display for CompanionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.found {
            None => write!(f, "herdr-nvim is not installed on {}", self.machine)?,
            Some(found) => write!(
                f,
                "herdr-nvim on {} is {} ({}), not {} ({})",
                self.machine, found.version, found.rev, self.expected.version, self.expected.rev
            )?,
        }
        write!(f, "; install it there: {}", install_command(&self.expected))
    }
}

impl std::error::Error for CompanionError {}

impl CompanionError {
    /// What the app's tree shows and toasts.
    pub fn to_json(&self) -> Value {
        json!({
            "machine": self.machine,
            "problem": self.problem,
            "found": self.found,
            "expected": self.expected,
            "install": install_command(&self.expected),
            "message": self.to_string(),
        })
    }
}

/// What one `hc exec` gave.
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Pure: the handshake's answer. A companion that ran but gave no identity
/// (one from before the handshake) is another build; hc failing to start it
/// is a missing one. Anything else is hc's own failure.
pub fn classify(machine: &str, this: &Identity, ran: &Ran) -> Result<()> {
    let error = |problem, found| CompanionError {
        machine: machine.to_owned(),
        problem,
        found,
        expected: this.clone(),
    };
    if ran.code == Some(0) {
        let found = serde_json::from_str::<Identity>(ran.stdout.trim()).ok();
        return match found {
            Some(found) if matches(this, &found) => Ok(()),
            found => Err(error(Problem::Mismatch, found).into()),
        };
    }
    let kind = serde_json::from_str::<Value>(ran.stderr.trim())
        .ok()
        .and_then(|v| v.pointer("/error/kind")?.as_str().map(str::to_owned));
    if kind.as_deref() == Some("spawn_failed") || ran.code == Some(127) {
        return Err(error(Problem::Missing, None).into());
    }
    if ran.stderr.starts_with("usage: herdr-nvim") {
        return Err(error(Problem::Mismatch, None).into());
    }
    bail!("{machine}: hc: {}", ran.stderr.trim())
}

/// Pure: `ctx`'s machine, once every id in it is that machine's.
fn machine_of(ctx: &cli::AppCtx) -> Result<&str> {
    let machine = remote::machine_of(ctx).context("not a remote tab")?;
    for id in [Some(&ctx.space), Some(&ctx.tab), ctx.terminal.as_ref()]
        .into_iter()
        .flatten()
    {
        remote::raw(machine, id)?;
    }
    Ok(machine)
}

/// Pure: the hc argv that runs `herdr-nvim <args>` on `machine`.
pub fn exec_argv(machine: &str, secs: u32, args: &[String]) -> Vec<String> {
    let mut argv: Vec<String> = ["exec", "--on", machine, "--timeout"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    argv.extend([secs.to_string(), "--".into(), "herdr-nvim".into()]);
    argv.extend(args.iter().cloned());
    argv
}

fn exec(machine: &str, secs: u32, args: &[String]) -> Result<Ran> {
    let out = Command::new(super::rpc::hc_bin())
        .args(exec_argv(machine, secs, args))
        .stdin(Stdio::null())
        .output()
        .context("cannot run hc")?;
    Ok(Ran {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Runs `herdr-nvim <args>` on `ctx`'s machine, after the handshake, and
/// returns what it printed.
pub fn delegate(ctx: &cli::AppCtx, args: &[String]) -> Result<String> {
    let machine = machine_of(ctx)?;
    classify(
        machine,
        &Identity::this_build(),
        &exec(machine, HANDSHAKE_SECS, &["app-version".to_owned()])?,
    )?;
    let ran = exec(machine, WORK_SECS, args)?;
    if ran.code != Some(0) {
        let reason = ran.stderr.trim();
        bail!(
            "{machine}: {}",
            reason.strip_prefix("herdr-nvim: ").unwrap_or(reason)
        );
    }
    Ok(ran.stdout)
}

/// Pure: `mode` and its `args` as the companion gets them, without `--ctx`
/// (its ids are this machine's view of the tab).
fn delegated(mode: &str, args: &[String]) -> Vec<String> {
    let mut out = vec![mode.to_owned()];
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--ctx" {
            rest.next();
        } else {
            out.push(arg.clone());
        }
    }
    out
}

/// Prints `herdr-nvim <mode> <args>`'s output: the companion's for a remote
/// `--ctx`, else `here`'s.
pub fn serve(mode: &str, args: &[String], here: impl FnOnce() -> Result<String>) -> Result<()> {
    let out = match cli::ctx_from_args(args)? {
        Some(ctx) if remote::is_remote(&ctx) => delegate(&ctx, &delegated(mode, args))?,
        _ => here()?,
    };
    print!("{out}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    fn id(version: &str, rev: &str) -> Identity {
        Identity {
            version: version.into(),
            rev: rev.into(),
        }
    }

    fn ran(code: i32, stdout: &str, stderr: &str) -> Ran {
        Ran {
            code: Some(code),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    fn problem(result: Result<()>) -> (Problem, Option<Identity>) {
        let error = result.unwrap_err();
        let error = error
            .downcast_ref::<CompanionError>()
            .expect("a companion error");
        (error.problem, error.found.clone())
    }

    #[test]
    fn only_the_same_clean_build_matches() {
        let this = id("1.3.1", COMMIT);
        assert!(matches(&this, &this.clone()));
        assert!(!matches(&this, &id("1.3.2", COMMIT)));
        assert!(!matches(&this, &id("1.3.1", &COMMIT.replace('0', "f"))));
        let dirty = id("1.3.1", &format!("{COMMIT}-dirty-17"));
        assert!(
            matches(&dirty, &dirty.clone()),
            "a dev build matches itself"
        );
        assert!(!matches(
            &dirty,
            &id("1.3.1", &format!("{COMMIT}-dirty-18"))
        ));
        assert!(!matches(&this, &dirty) && !matches(&dirty, &this));
        let unknown = id("1.3.1", "unknown");
        assert!(!matches(&unknown, &unknown.clone()));
    }

    #[test]
    fn the_install_command_pins_this_builds_commit() {
        assert_eq!(
            install_command(&id("1.3.1", COMMIT)),
            format!(
                "cargo install --locked --root ~/.local --git {REPO} --rev {COMMIT} herdr-nvim"
            )
        );
        assert!(install_command(&id("1.3.1", &format!("{COMMIT}-dirty-1"))).contains(COMMIT));
        assert!(!install_command(&id("1.3.1", "unknown")).contains("--rev"));
    }

    #[test]
    fn the_handshake_tells_missing_from_other_builds_from_hc_failures() {
        let this = id("1.3.1", COMMIT);
        let reported = |found: &Identity| ran(0, &serde_json::to_string(found).unwrap(), "");
        assert!(classify("box", &this, &reported(&this)).is_ok());
        let other = id("1.2.0", COMMIT);
        assert_eq!(
            problem(classify("box", &this, &reported(&other))),
            (Problem::Mismatch, Some(other))
        );
        assert_eq!(
            problem(classify("box", &this, &ran(0, "garbage", ""))),
            (Problem::Mismatch, None)
        );
        let spawn =
            r#"{"error":{"hint":"","kind":"spawn_failed","message":"cannot start herdr-nvim"}}"#;
        assert_eq!(
            problem(classify("box", &this, &ran(1, "", spawn))),
            (Problem::Missing, None)
        );
        assert_eq!(
            problem(classify(
                "box",
                &this,
                &ran(127, "", "sh: herdr-nvim: not found")
            )),
            (Problem::Missing, None)
        );
        assert_eq!(
            problem(classify(
                "box",
                &this,
                &ran(2, "", "usage: herdr-nvim <toggle|...>")
            )),
            (Problem::Mismatch, None)
        );
        let offline = classify("box", &this, &ran(1, "", "machine is unavailable")).unwrap_err();
        assert!(offline.downcast_ref::<CompanionError>().is_none());
        assert_eq!(offline.to_string(), "box: hc: machine is unavailable");
    }

    #[test]
    fn ids_of_another_machine_never_reach_it() {
        let ctx = |space: &str, tab: &str, terminal: Option<&str>| cli::AppCtx {
            space: space.into(),
            tab: tab.into(),
            terminal: terminal.map(str::to_owned),
            cwd: None,
        };
        assert_eq!(
            machine_of(&ctx("box/s1", "box/t1", Some("box/t7"))).unwrap(),
            "box"
        );
        assert!(machine_of(&ctx("other/s1", "box/t1", None)).is_err());
        assert!(machine_of(&ctx("box/s1", "box/t1", Some("t7"))).is_err());
        assert!(machine_of(&ctx("s1", "t1", None)).is_err());
        let args: Vec<String> = ["/x", "--ctx", "{}", "--limit", "9"]
            .map(String::from)
            .into();
        assert_eq!(
            delegated("app-list", &args),
            ["app-list", "/x", "--limit", "9"]
        );
        assert_eq!(
            exec_argv("box", 8, &["app-list".into(), "/x".into()]),
            [
                "exec",
                "--on",
                "box",
                "--timeout",
                "8",
                "--",
                "herdr-nvim",
                "app-list",
                "/x"
            ]
        );
    }
}
