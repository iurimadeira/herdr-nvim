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
/// PATH finds it (`~/.local/bin`); none for a build no companion can match
/// (local changes, or built outside git).
pub fn install_command(this: &Identity) -> Option<String> {
    this.commit()
        .filter(|commit| *commit == this.rev)
        .map(|commit| {
            format!("cargo install --locked --root ~/.local --git {REPO} --rev {commit} herdr-nvim")
        })
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

impl CompanionError {
    /// What is wrong on the machine: the toast's title.
    pub fn title(&self) -> String {
        let (machine, expected) = (&self.machine, &self.expected);
        match (self.problem, &self.found) {
            (Problem::Missing, _) => format!("herdr-nvim is not installed on {machine}"),
            (Problem::Mismatch, None) => format!(
                "herdr-nvim on {machine} is older than {} ({})",
                expected.version, expected.rev
            ),
            (Problem::Mismatch, Some(found)) => format!(
                "herdr-nvim on {machine} is {} ({}), not {} ({})",
                found.version, found.rev, expected.version, expected.rev
            ),
        }
    }

    /// What fixes it: the install command, or why no companion can match
    /// this build.
    pub fn remedy(&self) -> String {
        match install_command(&self.expected) {
            Some(install) => format!("install it there: {install}"),
            None if self.expected.commit().is_some() => {
                "this desktop build has uncommitted changes, so no companion matches it; use a build from a commit".to_owned()
            }
            None => "this desktop build is not from a git commit, so no companion matches it; use a build from a commit".to_owned(),
        }
    }

    /// What the app's tree shows (`message`) and toasts (`title`, `remedy`).
    pub fn to_json(&self) -> Value {
        json!({
            "machine": self.machine,
            "problem": self.problem,
            "found": self.found,
            "expected": self.expected,
            "install": install_command(&self.expected),
            "title": self.title(),
            "remedy": self.remedy(),
            "message": self.to_string(),
        })
    }
}

impl fmt::Display for CompanionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}; {}", self.title(), self.remedy())
    }
}

impl std::error::Error for CompanionError {}

/// What one `hc exec` gave.
pub struct Ran {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Pure: why a run failed: hc's own error message (`{"error": {message}}`
/// on stderr), else what the program wrote to stderr, else its exit code.
pub fn failure(ran: &Ran) -> String {
    let stderr = ran.stderr.trim();
    serde_json::from_str::<Value>(stderr)
        .ok()
        .and_then(|v| v.pointer("/error/message")?.as_str().map(str::to_owned))
        .or_else(|| {
            (!stderr.is_empty()).then(|| {
                stderr
                    .strip_prefix("herdr-nvim: ")
                    .unwrap_or(stderr)
                    .to_owned()
            })
        })
        .unwrap_or_else(|| match ran.code {
            Some(code) => format!("exit {code}"),
            None => "killed by a signal".to_owned(),
        })
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
    bail!("{machine}: {}", failure(ran))
}

/// Pure: `ctx`'s machine, and `ctx` as that machine's herdr-nvim names it
/// (its bare ids); an id of another machine, or a local one, is refused.
fn on_machine(ctx: &cli::AppCtx) -> Result<(&str, Value)> {
    let machine = remote::machine_of(ctx).context("not a remote tab")?;
    let terminal = ctx
        .terminal
        .as_deref()
        .map(|id| remote::raw(machine, id))
        .transpose()?;
    let bare = json!({
        "space": remote::raw(machine, &ctx.space)?,
        "tab": remote::raw(machine, &ctx.tab)?,
        "terminal": terminal,
        "cwd": ctx.cwd,
    });
    Ok((machine, bare))
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

/// Runs `herdr-nvim <args>` on `machine`, after the handshake, and returns
/// what it printed.
fn delegate(machine: &str, args: &[String]) -> Result<String> {
    classify(
        machine,
        &Identity::this_build(),
        &exec(machine, HANDSHAKE_SECS, &["app-version".to_owned()])?,
    )?;
    let ran = exec(machine, WORK_SECS, args)?;
    if ran.code != Some(0) {
        bail!("{machine}: {}", failure(&ran));
    }
    Ok(ran.stdout)
}

/// Pure: `mode` and its `args` as the companion gets them, `--ctx` being
/// `bare` (the tab as its machine names it).
fn delegated(mode: &str, args: &[String], bare: &Value) -> Vec<String> {
    let mut out = vec![mode.to_owned()];
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        out.push(arg.clone());
        if arg == "--ctx" {
            rest.next();
            out.push(bare.to_string());
        }
    }
    out
}

/// Prints `herdr-nvim <mode> <args>`'s output: the companion's for a remote
/// `--ctx`, else `here`'s.
pub fn serve(mode: &str, args: &[String], here: impl FnOnce() -> Result<String>) -> Result<()> {
    let out = match cli::ctx_from_args(args)? {
        Some(ctx) if remote::is_remote(&ctx) => {
            let (machine, bare) = on_machine(&ctx)?;
            delegate(machine, &delegated(mode, args, &bare))?
        }
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
    fn only_a_build_a_companion_can_match_offers_an_install_command() {
        assert_eq!(
            install_command(&id("1.3.1", COMMIT)).unwrap(),
            format!(
                "cargo install --locked --root ~/.local --git {REPO} --rev {COMMIT} herdr-nvim"
            )
        );
        assert_eq!(
            install_command(&id("1.3.1", &format!("{COMMIT}-dirty-1"))),
            None
        );
        assert_eq!(install_command(&id("1.3.1", "unknown")), None);
        let error = |rev: &str| CompanionError {
            machine: "box".into(),
            problem: Problem::Missing,
            found: None,
            expected: id("1.3.1", rev),
        };
        let dirty = error(&format!("{COMMIT}-dirty-1"));
        assert_eq!(dirty.title(), "herdr-nvim is not installed on box");
        assert!(dirty.remedy().contains("uncommitted changes"));
        assert!(!dirty.to_string().contains("cargo install"));
        assert_eq!(dirty.to_json()["install"], Value::Null);
        let unknown = error("unknown");
        assert!(unknown.remedy().contains("not from a git commit"));
        assert_eq!(unknown.to_json()["install"], Value::Null);
        let clean = error(COMMIT).to_json();
        assert_eq!(clean["title"], "herdr-nvim is not installed on box");
        assert_eq!(
            clean["message"],
            format!(
                "herdr-nvim is not installed on box; install it there: {}",
                clean["install"].as_str().unwrap()
            )
        );
    }

    #[test]
    fn a_failed_run_says_why_by_hcs_message_else_stderr_else_its_exit() {
        let unknown_machine = r#"{"error":{"hint":"see hc machines","kind":"unknown_machine","message":"no machine named box"}}"#;
        assert_eq!(
            failure(&ran(4, "", unknown_machine)),
            "no machine named box"
        );
        assert_eq!(
            failure(&ran(1, "", "herdr-nvim: not a folder: /x\n")),
            "not a folder: /x"
        );
        assert_eq!(failure(&ran(255, "", "  ")), "exit 255");
        let killed = Ran {
            code: None,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert_eq!(failure(&killed), "killed by a signal");
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
        let old = classify("box", &this, &ran(2, "", "usage: herdr-nvim <toggle|...>"));
        assert!(old
            .unwrap_err()
            .to_string()
            .starts_with("herdr-nvim on box is older than 1.3.1"),);
        let gone = classify("box", &this, &ran(1, "", spawn));
        assert!(gone
            .unwrap_err()
            .to_string()
            .starts_with("herdr-nvim is not installed on box; install it there: cargo install"),);
        let offline = classify("box", &this, &ran(1, "", "machine is unavailable")).unwrap_err();
        assert!(offline.downcast_ref::<CompanionError>().is_none());
        assert_eq!(offline.to_string(), "box: machine is unavailable");
        let unknown =
            r#"{"error":{"hint":"","kind":"unknown_machine","message":"no machine named box"}}"#;
        assert_eq!(
            classify("box", &this, &ran(4, "", unknown))
                .unwrap_err()
                .to_string(),
            "box: no machine named box"
        );
    }

    #[test]
    fn ids_of_another_machine_never_reach_it() {
        let ctx = |space: &str, tab: &str, terminal: Option<&str>| cli::AppCtx {
            space: space.into(),
            tab: tab.into(),
            terminal: terminal.map(str::to_owned),
            cwd: Some("/x".into()),
        };
        let remote = ctx("box/s1", "box/t1", Some("box/t7"));
        let (machine, bare) = on_machine(&remote).unwrap();
        assert_eq!(machine, "box");
        assert_eq!(
            bare,
            json!({"space": "s1", "tab": "t1", "terminal": "t7", "cwd": "/x"})
        );
        assert!(on_machine(&ctx("other/s1", "box/t1", None)).is_err());
        assert!(on_machine(&ctx("box/s1", "box/t1", Some("t7"))).is_err());
        assert!(on_machine(&ctx("box/s1", "t1", None)).is_err());
        assert!(on_machine(&ctx("s1", "t1", None)).is_err());
        let args: Vec<String> = ["/x", "--ctx", "{}", "--limit", "9"]
            .map(String::from)
            .into();
        assert_eq!(
            delegated("app-list", &args, &json!({"space": "s1", "tab": "t1"})),
            [
                "app-list",
                "/x",
                "--ctx",
                r#"{"space":"s1","tab":"t1"}"#,
                "--limit",
                "9"
            ]
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
