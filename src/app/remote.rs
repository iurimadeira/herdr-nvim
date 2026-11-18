//! Remote editor panes: local window commands, remote Neovim and supervision.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{cli, doc, node, sidebar};
use crate::config::{self, Sidebar, SidebarPosition};

const WORKER: &str = include_str!("../../lua/herdr-nvim/remote.lua");
const BOOTSTRAP: &str = r#"
local u = vim.uv
assert(u.os_uname().sysname ~= 'Windows_NT', 'Remote Neovim requires a POSIX host')
local root = u.os_tmpdir() .. '/herdr-nvim-remote-' .. u.getuid()
local stat = u.fs_lstat(root)
if not stat then u.fs_mkdir(root, 448); stat = u.fs_lstat(root) end
assert(stat and stat.type == 'directory' and stat.uid == u.getuid(), 'Unsafe remote runtime directory')
assert(u.fs_chmod(root, 448))
local worker = root .. '/worker-' .. sha .. '.lua'
local stage
if not u.fs_stat(worker) then
  stage = root .. '/.upload-' .. u.os_getpid() .. '-' .. u.hrtime()
  assert(io.open(stage, 'wb')):close()
end
io.stdout:write(vim.json.encode({worker=worker,stage=stage}) .. '\n')
"#;

pub(crate) fn machine(id: &str) -> Option<&str> {
    id.split_once('/').map(|(machine, _)| machine)
}

/// The one rule for a context's machine: its tab's, else its space's. A
/// context whose IDs name different machines (or one local) stays remote, so
/// `Request::new` refuses it instead of running it locally.
pub(crate) fn machine_of(ctx: &cli::AppCtx) -> Option<&str> {
    machine(&ctx.tab).or_else(|| machine(&ctx.space))
}

pub(crate) fn is_remote(ctx: &cli::AppCtx) -> bool {
    machine_of(ctx).is_some()
}

pub(crate) fn require_local(ctx: &cli::AppCtx) -> Result<()> {
    if is_remote(ctx) {
        bail!(
            "this command is not available for remote tabs; use Neovim's own explorer and plugins"
        );
    }
    Ok(())
}

fn raw<'a>(machine: &str, id: &'a str) -> Result<&'a str> {
    let (prefix, id) = id
        .split_once('/')
        .context("remote IDs need a machine prefix")?;
    if prefix != machine || id.is_empty() || id.contains('/') || machine.is_empty() {
        bail!("ID belongs to a different machine or is malformed");
    }
    Ok(id)
}

fn digest(bytes: &[u8]) -> String {
    crate::osfs::sha256_hex(bytes)
}

#[derive(Serialize)]
struct Request {
    space: String,
    tab: String,
    key: String,
    nvim: String,
    env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    col: Option<u32>,
}

impl Request {
    fn new(ctx: &cli::AppCtx, machine: &str, settings: &Sidebar) -> Result<Self> {
        let space = raw(machine, &ctx.space)?.to_owned();
        let tab = raw(machine, &ctx.tab)?.to_owned();
        let terminal = ctx
            .terminal
            .as_deref()
            .map(|t| raw(machine, t).map(str::to_owned))
            .transpose()?;
        let overrides = settings.env_override();
        Ok(Self {
            key: digest(format!("{space}:{tab}").as_bytes())[..20].to_owned(),
            space,
            tab,
            terminal,
            cwd: ctx.cwd.clone(),
            nvim: settings.nvim_bin.clone(),
            env: overrides
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            file: None,
            line: None,
            col: None,
        })
    }
}

#[derive(Deserialize)]
struct Info {
    doc: Value,
    terminals: Vec<RemoteTerminal>,
}

#[derive(Deserialize)]
struct RemoteTerminal {
    address: String,
    state: String,
    labels: BTreeMap<String, String>,
}

struct Session {
    machine: String,
    request: Request,
    position: SidebarPosition,
    script: String,
    _lock: File,
}

impl Session {
    fn new(ctx: &cli::AppCtx) -> Result<Self> {
        let machine = machine_of(ctx).context("not a remote tab")?.to_owned();
        let settings = config::load().remote_sidebar(&machine);
        let request = Request::new(ctx, &machine, &settings)?;
        let lock = lock(&format!("{machine}:{}:{}", request.space, request.tab))?;
        let script = deploy(&machine, &request)?;
        Ok(Self {
            machine,
            request,
            position: settings.position,
            script,
            _lock: lock,
        })
    }

    fn qualify(&self, id: &str) -> String {
        format!("{}/{id}", self.machine)
    }

    fn call(&self, operation: &str) -> Result<Value> {
        let text = execute(
            &self.machine,
            &self.request,
            &[
                self.request.nvim.clone(),
                "--clean".into(),
                "-l".into(),
                self.script.clone(),
                operation.into(),
                serde_json::to_string(&self.request)?,
            ],
        )?;
        serde_json::from_str(&text).context("remote Neovim returned invalid JSON")
    }

    fn info(&self) -> Result<(doc::Doc, Option<String>)> {
        let info: Info = serde_json::from_value(self.call("info")?)?;
        let doc = doc::parse(&info.doc).context("remote layout document is unavailable")?;
        if doc::tab(&doc, &self.request.tab).map(|(s, _)| s.id.as_str())
            != Some(self.request.space.as_str())
        {
            bail!("remote tab does not belong to the requested space");
        }
        Ok((doc, ui_screen(&info.terminals)?))
    }

    fn hide(&self, doc: &doc::Doc, screen: Option<&str>) -> Result<Value> {
        let Some(screen) = shown(doc, screen) else {
            return Ok(json!({}));
        };
        if !can_hide(doc, &self.request.tab)? {
            bail!("cannot hide the remote tab's only pane; split another pane first");
        }
        let address = self.qualify(screen);
        cli::app_do("terminal.park", &json!({"terminal": address}))?;
        wait_for(|| Ok(!cli::app_listing()?.contains_terminal(&address)))?;
        Ok(json!({"closed": address}))
    }

    fn show(
        &self,
        doc: &doc::Doc,
        screen: Option<&str>,
        state: &Value,
        focus: bool,
    ) -> Result<Value> {
        let mut terminal = None;
        match placement(doc, &self.request.tab, screen)? {
            Placement::Shown(screen) => terminal = Some(screen),
            Placement::Reattach {
                screen,
                anchor,
                anchor_pane,
            } => {
                cli::app_do("pane.focus", &json!({"terminal": self.qualify(&anchor)}))?;
                let pane = self.qualify(&anchor_pane);
                wait_for(|| {
                    Ok(cli::app_status()?
                        .get("focused_pane")
                        .and_then(Value::as_str)
                        == Some(pane.as_str()))
                })?;
                cli::app_do(
                    "terminal.keep",
                    &json!({"terminal": self.qualify(&screen), "place": sidebar::direction(self.position)}),
                )?;
                terminal = Some(screen);
            }
            Placement::Replace(screen) => self.kill(&screen)?,
            Placement::Missing => {}
        }
        if terminal.is_none() {
            let root = self
                .qualify(&doc::root_of(doc, &self.request.tab).context("missing remote tab root")?);
            let mut argv = vec!["env".to_owned()];
            argv.extend(
                self.request
                    .env
                    .iter()
                    .map(|(key, value)| format!("{key}={value}")),
            );
            argv.extend([
                state["nvim"]
                    .as_str()
                    .context("missing remote Neovim executable")?
                    .to_owned(),
                "--clean".into(),
                "--server".into(),
                state["socket"]
                    .as_str()
                    .context("missing remote Neovim socket")?
                    .to_owned(),
                "--remote-ui".into(),
            ]);
            let made = cli::app_do(
                "pane.split",
                &json!({"pane": root, "direction": sidebar::direction(self.position), "argv": argv}),
            )?;
            let address = made["terminal"]
                .as_str()
                .context("remote split returned no terminal")?;
            let id = raw(&self.machine, address)?.to_owned();
            if let Err(error) = hc(&[
                "label".into(),
                address.into(),
                format!("herdr-nvim.sidebar={}", self.request.tab),
                format!("herdr-nvim.remote={}", self.request.key),
                "herdr-nvim.remote-role=ui".into(),
            ]) {
                if let Err(cleanup) = self.kill(&id) {
                    eprintln!("herdr-nvim: UI cleanup failed: {cleanup}");
                }
                return Err(error).context("could not label the remote Neovim UI client");
            }
            terminal = Some(id);
        }
        let address = self.qualify(&terminal.context("no remote UI client")?);
        wait_for(|| Ok(cli::app_listing()?.contains_terminal(&address)))?;
        if focus {
            cli::app_do("pane.focus", &json!({"terminal": address}))?;
        }
        Ok(json!({"terminal": address}))
    }

    fn kill(&self, terminal: &str) -> Result<()> {
        hc(&["kill".into(), self.qualify(terminal)]).map(|_| ())
    }
}

/// Pure: the tab's one live Neovim UI client, refusing several.
fn ui_screen(terminals: &[RemoteTerminal]) -> Result<Option<String>> {
    let mut screens = terminals.iter().filter(|t| {
        t.state == "live"
            && t.labels
                .get("herdr-nvim.remote-role")
                .is_some_and(|role| role == "ui")
    });
    let screen = screens.next().map(|t| t.address.clone());
    if screens.next().is_some() {
        bail!("the remote tab has multiple Neovim UI clients; close the extra clients first");
    }
    Ok(screen)
}

/// Pure: the UI client when a pane shows it.
fn shown<'a>(doc: &doc::Doc, screen: Option<&'a str>) -> Option<&'a str> {
    screen.filter(|screen| doc::pane_of(doc, screen).is_some())
}

/// Pure: whether the tab keeps a pane (a terminal or a mod) once its editor
/// is hidden; hiding its only pane would close the tab.
fn can_hide(doc: &doc::Doc, tab: &str) -> Result<bool> {
    let tab = doc::tab(doc, tab).context("missing remote tab")?.1;
    let panes = node::walk(&tab.root)
        .filter(|n| {
            node::terminal(n).is_some()
                || n.pointer("/content/mod").and_then(Value::as_str).is_some()
        })
        .count();
    Ok(panes >= 2)
}

#[derive(Debug, PartialEq)]
enum Toggle {
    Hide,
    Show,
}

/// Pure: what a toggle does with the editor shown or not.
fn toggle_action(want: sidebar::Want, shown: bool) -> Toggle {
    if want == sidebar::Want::Close || (want == sidebar::Want::Flip && shown) {
        Toggle::Hide
    } else {
        Toggle::Show
    }
}

/// Where showing the editor starts from.
#[derive(Debug, PartialEq)]
enum Placement {
    /// A pane shows the UI client already.
    Shown(String),
    /// A parked client goes back beside the tab's only terminal.
    Reattach {
        screen: String,
        anchor: String,
        anchor_pane: String,
    },
    /// A parked client with no single anchor is ended; a new one is split.
    Replace(String),
    /// No client: a new one is split.
    Missing,
}

/// Pure: the placement for the tab's UI client `screen`.
fn placement(doc: &doc::Doc, tab: &str, screen: Option<&str>) -> Result<Placement> {
    let Some(screen) = screen else {
        return Ok(Placement::Missing);
    };
    if doc::pane_of(doc, screen).is_some() {
        return Ok(Placement::Shown(screen.to_owned()));
    }
    let own = doc::terminals_of(doc, tab);
    if let [anchor] = own.as_slice() {
        let anchor_pane = doc::pane_of(doc, anchor).context("missing anchor pane")?.0;
        return Ok(Placement::Reattach {
            screen: screen.to_owned(),
            anchor: anchor.clone(),
            anchor_pane,
        });
    }
    Ok(Placement::Replace(screen.to_owned()))
}

fn hc(args: &[String]) -> Result<String> {
    let output = Command::new(super::rpc::hc_bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("cannot run hc")?;
    if !output.status.success() {
        bail!("hc: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    String::from_utf8(output.stdout).context("hc returned invalid UTF-8")
}

fn execute(machine: &str, request: &Request, args: &[String]) -> Result<String> {
    let mut command: Vec<String> = ["exec", "--on", machine, "--timeout", "20", "--", "env"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    command.extend(
        request
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    command.extend_from_slice(args);
    hc(&command).with_context(|| format!("remote Neovim on {machine}"))
}

/// The `-c` command running `code` in a headless Neovim. Neovim exits 0 when
/// a `-c` command fails and prints the error nowhere we read, so a failure
/// is written to stdout as `{"error": message}` instead.
fn helper_command(code: &str) -> String {
    format!(
        "lua local ok,e=pcall(function()\n{code}\nend) if not ok then io.stdout:write(vim.json.encode({{error=tostring(e)}})..'\\n') end"
    )
}

/// Pure: a helper's stdout to its value, or the helper's own error.
fn helper_result(text: &str) -> Result<Value> {
    let value: Value =
        serde_json::from_str(text).context("remote deployment returned invalid JSON")?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        bail!("remote helper failed: {error}");
    }
    Ok(value)
}

fn lua(machine: &str, request: &Request, code: &str) -> Result<Value> {
    let text = execute(
        machine,
        request,
        &[
            request.nvim.clone(),
            "--clean".into(),
            "--headless".into(),
            "-c".into(),
            helper_command(code),
            "-c".into(),
            "qa".into(),
        ],
    )?;
    helper_result(&text).with_context(|| format!("remote Neovim on {machine}"))
}

fn deploy(machine: &str, request: &Request) -> Result<String> {
    let sha = digest(WORKER.as_bytes());
    let deployed = lua(
        machine,
        request,
        &format!("local sha={};{BOOTSTRAP}", serde_json::to_string(&sha)?),
    )?;
    let worker = deployed["worker"]
        .as_str()
        .context("missing remote worker path")?
        .to_owned();
    if let Some(stage) = deployed.get("stage").and_then(Value::as_str) {
        // Some machine transports have restrictive request-line limits.
        for text in upload_chunks(WORKER) {
            lua(machine, request, &format!("local f=assert(io.open({},'ab'));f:write({});f:close();io.stdout:write('true\\n')", serde_json::to_string(stage)?, serde_json::to_string(text)?))?;
        }
        lua(machine, request, &format!("local f=assert(io.open({},'rb'));local d=f:read('*a');f:close();assert(vim.fn.sha256(d)=='{sha}','Incomplete helper upload');assert(vim.uv.fs_rename({},{}));io.stdout:write('true\\n')", serde_json::to_string(stage)?, serde_json::to_string(stage)?, serde_json::to_string(&worker)?))?;
    }
    Ok(worker)
}

fn upload_chunks(mut text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    while !text.is_empty() {
        let mut end = text.len().min(768);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&text[..end]);
        text = &text[end..];
    }
    chunks
}

fn lock(key: &str) -> Result<File> {
    let root = crate::state::state_dir().join("remote-locks");
    fs::create_dir_all(&root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(digest(key.as_bytes())))?;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < until => {
                thread::sleep(Duration::from_millis(50))
            }
            Err(error) => {
                return Err(error).context("another remote editor action is running for this tab")
            }
        }
    }
}

fn wait_for(mut check: impl FnMut() -> Result<bool>) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(3);
    while !check()? {
        if Instant::now() >= until {
            bail!("the app did not finish changing the remote pane");
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub(crate) fn open(
    ctx: &cli::AppCtx,
    file: String,
    line: Option<u32>,
    col: Option<u32>,
    focus: bool,
) -> Result<Value> {
    let mut session = Session::new(ctx)?;
    let (doc, screen) = session.info()?;
    session.request.file = Some(file);
    session.request.line = line;
    session.request.col = col;
    let state = session.call("open")?;
    session.show(&doc, screen.as_deref(), &state, focus)
}

pub(crate) fn toggle(ctx: &cli::AppCtx, want: sidebar::Want) -> Result<Value> {
    let session = Session::new(ctx)?;
    let (doc, screen) = session.info()?;
    match toggle_action(want, shown(&doc, screen.as_deref()).is_some()) {
        Toggle::Hide => session.hide(&doc, screen.as_deref()),
        Toggle::Show => session.show(&doc, screen.as_deref(), &session.call("open")?, true),
    }
}

pub(crate) fn current(ctx: &cli::AppCtx) -> Result<Value> {
    Session::new(ctx)?.call("current")
}

pub(crate) fn stop_cmd() -> Result<()> {
    let args: Vec<String> = crate::argv::args().skip(2).collect();
    let session = Session::new(&cli::target_ctx(&args)?)?;
    session.call("stop")?;
    println!("{}", json!({"stopped": true}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> cli::AppCtx {
        cli::AppCtx {
            space: "box/s1".into(),
            tab: "box/t1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn remote_ids_are_machine_scoped_before_any_side_effect() {
        let context = cli::AppCtx {
            terminal: Some("box/t7".into()),
            ..ctx()
        };
        let request = Request::new(&context, "box", &Sidebar::default()).unwrap();
        assert_eq!(
            (
                &request.space[..],
                &request.tab[..],
                request.terminal.as_deref()
            ),
            ("s1", "t1", Some("t7"))
        );
        for wrong in ["t7", "other/t7", "box/", "box/other/t7"] {
            assert!(raw("box", wrong).is_err());
        }
        assert!(Request::new(
            &cli::AppCtx {
                tab: "other/t1".into(),
                ..context
            },
            "box",
            &Sidebar::default()
        )
        .is_err());
    }

    #[test]
    fn absent_paths_are_omitted_instead_of_lua_null_values() {
        let request =
            serde_json::to_value(Request::new(&ctx(), "box", &Sidebar::default()).unwrap())
                .unwrap();
        for key in ["cwd", "terminal", "file", "line", "col"] {
            assert!(request.get(key).is_none());
        }
    }

    #[test]
    fn malformed_remote_environment_entries_are_skipped_like_local_ones() {
        let settings = Sidebar {
            nvim_env: vec!["BAD KEY=private-value".into(), "NVIM_APPNAME=work".into()],
            ..Default::default()
        };
        let request = Request::new(&ctx(), "box", &settings).unwrap();
        assert_eq!(
            request.env,
            BTreeMap::from([("NVIM_APPNAME".to_owned(), "work".to_owned())])
        );
    }

    #[test]
    fn remote_environment_overrides_keep_the_last_value() {
        let settings = Sidebar {
            nvim_env: vec!["NVIM_APPNAME=old".into(), "NVIM_APPNAME=work".into()],
            ..Default::default()
        };
        assert_eq!(
            Request::new(&ctx(), "box", &settings).unwrap().env["NVIM_APPNAME"],
            "work"
        );
    }

    #[test]
    fn uploads_preserve_unicode_without_exceeding_the_byte_limit() {
        let source = format!("{}雪😀{}", "a".repeat(767), "雪".repeat(500));
        let chunks = upload_chunks(&source);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 768));
        assert_eq!(chunks.concat(), source);
    }

    #[test]
    fn a_context_is_remote_by_its_tab_else_its_space_and_mixed_ones_fail_closed() {
        let local = cli::AppCtx {
            space: "s1".into(),
            tab: "t1".into(),
            ..Default::default()
        };
        assert_eq!(machine_of(&local), None);
        assert_eq!(machine_of(&ctx()), Some("box"));
        for (space, tab) in [("box/s1", "t1"), ("s1", "box/t1"), ("other/s1", "box/t1")] {
            let mixed = cli::AppCtx {
                space: space.into(),
                tab: tab.into(),
                ..Default::default()
            };
            let machine = machine_of(&mixed).expect("a mixed context stays remote");
            assert!(Request::new(&mixed, machine, &Sidebar::default()).is_err());
        }
    }

    fn terminal(address: &str, state: &str, role: &str) -> RemoteTerminal {
        RemoteTerminal {
            address: address.into(),
            state: state.into(),
            labels: [("herdr-nvim.remote-role".to_owned(), role.to_owned())].into(),
        }
    }

    #[test]
    fn the_ui_screen_is_the_one_live_ui_client() {
        assert_eq!(ui_screen(&[]).unwrap(), None);
        let screens = [
            terminal("t5", "live", "daemon"),
            terminal("t6", "ended", "ui"),
            terminal("t9", "live", "ui"),
        ];
        assert_eq!(ui_screen(&screens).unwrap().as_deref(), Some("t9"));
        let two = [terminal("t8", "live", "ui"), terminal("t9", "live", "ui")];
        assert!(ui_screen(&two)
            .unwrap_err()
            .to_string()
            .contains("multiple Neovim UI clients"));
    }

    fn layout(children: Value) -> doc::Doc {
        doc::parse(&json!({"version":1,"value":{"spaces":[{"id":"s1","tabs":[
            {"id":"t1","root":{"id":"root","children":children}}]}]}}))
        .unwrap()
    }

    #[test]
    fn only_a_tab_with_another_pane_can_hide_its_editor() {
        let editor = json!({"id":"p9","content":{"terminal":"t9"}});
        assert!(!can_hide(&layout(json!([editor])), "t1").unwrap());
        let with_shell = layout(json!([{"id":"p7","content":{"terminal":"t7"}}, editor]));
        assert!(can_hide(&with_shell, "t1").unwrap());
        let with_mod = layout(json!([{"id":"pm","content":{"mod":"board"}}, editor]));
        assert!(can_hide(&with_mod, "t1").unwrap());
        assert!(can_hide(&with_mod, "t2").is_err());
    }

    #[test]
    fn a_toggle_hides_only_when_asked_or_shown() {
        use sidebar::Want::*;
        assert_eq!(toggle_action(Close, false), Toggle::Hide);
        assert_eq!(toggle_action(Close, true), Toggle::Hide);
        assert_eq!(toggle_action(Flip, true), Toggle::Hide);
        assert_eq!(toggle_action(Flip, false), Toggle::Show);
        assert_eq!(toggle_action(Open, true), Toggle::Show);
        assert_eq!(toggle_action(Open, false), Toggle::Show);
    }

    #[test]
    fn a_parked_client_reattaches_beside_the_only_terminal_else_is_replaced() {
        let shell = json!({"id":"p7","content":{"terminal":"t7"}});
        let editor = json!({"id":"p9","content":{"terminal":"t9"}});
        let shown_doc = layout(json!([shell, editor]));
        assert_eq!(
            placement(&shown_doc, "t1", None).unwrap(),
            Placement::Missing
        );
        assert_eq!(
            placement(&shown_doc, "t1", Some("t9")).unwrap(),
            Placement::Shown("t9".into())
        );
        assert_eq!(shown(&shown_doc, Some("t9")), Some("t9"));
        let parked = layout(json!([shell]));
        assert_eq!(shown(&parked, Some("t9")), None);
        assert_eq!(
            placement(&parked, "t1", Some("t9")).unwrap(),
            Placement::Reattach {
                screen: "t9".into(),
                anchor: "t7".into(),
                anchor_pane: "p7".into()
            }
        );
        let two = layout(json!([shell, {"id":"p8","content":{"terminal":"t8"}}]));
        assert_eq!(
            placement(&two, "t1", Some("t9")).unwrap(),
            Placement::Replace("t9".into())
        );
    }

    #[test]
    fn a_helper_error_is_reported_instead_of_invalid_json() {
        assert_eq!(helper_result("true\n").unwrap(), json!(true));
        let error = helper_result(r#"{"error":"Unsafe remote runtime directory"}"#).unwrap_err();
        assert!(error
            .to_string()
            .contains("Unsafe remote runtime directory"));
        assert!(helper_result("").is_err());
    }

    /// The real Neovim: a failing `-c` command still exits 0, so the
    /// wrapper must carry the message on stdout.
    #[cfg(unix)]
    #[test]
    fn a_failing_helper_reaches_stdout_from_a_real_neovim() {
        let run = |code: &str| {
            Command::new("nvim")
                .args([
                    "--clean",
                    "--headless",
                    "-c",
                    &helper_command(code),
                    "-c",
                    "qa",
                ])
                .stdin(Stdio::null())
                .output()
        };
        let Ok(failed) = run("assert(false, 'Remote Neovim requires a POSIX host')") else {
            eprintln!("skipped: no nvim on PATH");
            return;
        };
        assert!(failed.status.success(), "nvim itself fails the run");
        let error = helper_result(&String::from_utf8_lossy(&failed.stdout)).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Remote Neovim requires a POSIX host"),
            "{error}"
        );
        let ok = run("io.stdout:write(vim.json.encode({worker='w'})..'\\n')").unwrap();
        assert_eq!(
            helper_result(&String::from_utf8_lossy(&ok.stdout)).unwrap(),
            json!({"worker": "w"})
        );
    }
}
