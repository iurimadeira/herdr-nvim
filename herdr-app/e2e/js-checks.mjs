// Checks of the mod's pure parts, outside the app: `node herdr-app/e2e/js-checks.mjs`
// (run.sh runs it first).
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { register } from "node:module";
import { wanted } from "../mods/herdr-nvim/sync-rule.js";
import { ACTIONS, helpGroups, recipeTable } from "../mods/herdr-nvim/tree-keys.js";

const opts = { native: true, local: true, kept: false, hidByTreeKey: false };
const tab = (files, nvim, other = false) => ({ tab: "a", files, other, nvim });

// A tab switch: the panel follows the tab's nvim.
assert.deepEqual(wanted(tab(false, true), { ...tab(true, true), tab: "b" }, opts), { panel: "show" });
assert.deepEqual(wanted(tab(true, false), null, opts), { panel: "hide" });
assert.deepEqual(wanted(tab(false, true), null, { ...opts, kept: true }), { panel: null });
// The panel opened or closed on the same tab: nvim follows it.
assert.deepEqual(wanted(tab(true, false), tab(false, false), opts), { kept: false, nvim: true });
// Files right after a tab switch, seen in one look: opens that tab's nvim.
assert.deepEqual(wanted(tab(true, false), { ...tab(false, true), tab: "b" }, opts), { kept: false, nvim: true });
assert.deepEqual(wanted(tab(false, true), tab(true, true), opts), { nvim: true });
assert.deepEqual(wanted(tab(false, true), tab(true, true), { ...opts, hidByTreeKey: true }), { kept: true });
assert.deepEqual(wanted(tab(false, true, true), tab(true, true), opts), { nvim: false });
// nvim appeared or went: the panel follows it; nvim gone clears the mark.
assert.deepEqual(wanted(tab(false, true), tab(false, false), opts), { panel: "show" });
assert.deepEqual(wanted(tab(true, false), tab(true, true), { ...opts, kept: true }), { kept: false, panel: "hide" });
assert.deepEqual(wanted(tab(true, true), tab(true, true), opts), {});
// tree = "nvim": the panel never stays.
assert.deepEqual(wanted(tab(true, true), null, { ...opts, native: false }), { panel: "hide" });
assert.deepEqual(wanted(tab(true, false), tab(false, false), { ...opts, native: false }), { panel: "hide" });

// The key table: every key one action, the list and the recipe from it.
assert.equal(ACTIONS.get("j"), "down");
assert.equal(ACTIONS.get("r"), "rename");
assert.ok(helpGroups("space e").find(([g]) => g === "Navigate")[1].some(([k]) => k === "space e"));
assert.ok(recipeTable().startsWith("| Key | Does |"));

// --- The mod's files that use the app, on a stand-in `herdr` -----------------
// (herdr-stub.mjs: each check says what the app reports).
register(`data:text/javascript,${encodeURIComponent(`
  export async function resolve(spec, ctx, next) {
    if (["herdr", "gpui-kit", "gpui-base"].includes(spec)) return { url: ${JSON.stringify(new URL("./herdr-stub.mjs", import.meta.url).href)}, shortCircuit: true };
    return next(spec, ctx);
  }`)}`);
const { app } = await import("./herdr-stub.mjs");
const { markOf, sourceFor } = await import("../mods/herdr-nvim/tree.js");
const { TreeModel, fatalListing } = await import("../mods/herdr-nvim/tree-model.js");
const { panelState } = await import("../mods/herdr-nvim/panel.js");
const { request } = await import("../mods/herdr-nvim/ops.js");
const path = await import("../mods/herdr-nvim/path.js");
const { Sync } = await import("../mods/herdr-nvim/sync.js");
const ok = (stdout) => ({ code: 0, stdout, stderr: "" });

// Git marks are herdr-nvim's `app-git-status` (src/app/list.rs runs git):
// an untracked folder comes once, as `dir/`; it and the files in it are new.
// Outside a repo, or when it fails, there are none.
const here = sourceFor(null);
const status = " M src/a.js\0?? newdir/\0A  b.js\0 D gone.txt\0UU c.js\0";
app.exec = async (cmd, args) => {
  assert.deepEqual(args, ["app-git-status", "/r"], "a local tab's marks are read here, with no --ctx");
  return ok(JSON.stringify({ top: "/r", status }));
};
const marks = await here.marks("/r");
assert.deepEqual(marks, { "/r/src/a.js": "M", "/r/newdir": "A", "/r/b.js": "A", "/r/gone.txt": "D", "/r/c.js": "U" });
assert.equal(markOf(marks, "/r/newdir"), "A");
assert.equal(markOf(marks, "/r/newdir/deep/x.md"), "A");
assert.equal(markOf(marks, "/r/src/a.js"), "M");
assert.equal(markOf(marks, "/r/src/other.js"), null);
app.exec = async () => ok(JSON.stringify({ top: null, status: "" }));
assert.deepEqual(await here.marks("/elsewhere"), {});
app.exec = async () => ok("not json");
assert.deepEqual(await here.marks("/r"), {});
app.exec = async () => ({ code: 1, stdout: "", stderr: "herdr-nvim: boom\n" });
assert.deepEqual(await here.marks("/r"), {});

// A folder's listing is herdr-nvim's (`app-list`, src/app/list.rs, which
// sorts it): its entries in its order, each saying whether git ignores it.
const entry = (name, dir = false, ignored = false) => ({ name, dir, hidden: name.startsWith("."), ignored });
const listing = (...entries) => ok(JSON.stringify({ entries, truncated: false }));
const bin = "/nonexistent-app-home/mods/herdr-nvim/bin/herdr-nvim";
app.exec = async (cmd, args) => (cmd === bin && args[0] === "app-list" && args[1] === "/r"
  ? listing(entry("build", true, true), entry("src", true), entry(".env"), entry("a.txt", false, true)) : ok(""));
const r = await here.list("/r");
assert.deepEqual(r.map((e) => e.name), ["build", "src", ".env", "a.txt"]);
assert.deepEqual(r.filter((e) => e.ignored).map((e) => e.name), ["build", "a.txt"]);
app.exec = async () => ({ code: 1, stdout: "", stderr: "herdr-nvim: not a folder: /gone\n" });
await assert.rejects(here.list("/gone"), /^Error: not a folder: \/gone$/);

// The tree model: a watch batch lists again only the folders on screen it
// touches; a build in an ignored folder lists nothing; the same listing
// again changes no rows.
const listings = {
  "/r": listing(entry("src", true), entry("target", true, true), entry(".env"), entry("x.md")),
  "/r/src": listing(entry("a.js")),
  "/r/target": listing(entry("out", false, true)),
};
let listed = [];
app.exec = async (cmd, args) => {
  if (cmd === bin && args[0] === "app-list") { listed.push(args[1]); return listings[args[1]]; }
  return args[0] === "rev-parse" ? ok("/r\n") : ok("");
};
const model = new TreeModel();
model.reset("/r");
await model.reload();
assert.deepEqual(model.rows().map((r) => r.abs), ["/r/src", "/r/x.md"]); // target/ ignored and .env hidden: not shown
await model.toggle("/r/src");
assert.deepEqual(model.rows().map((r) => r.abs), ["/r/src", "/r/src/a.js", "/r/x.md"]);
listed = [];
assert.deepEqual(await model.reloadChanged(["/r/target/out", "/r/.git/index"]), { marks: "none" });
assert.deepEqual(listed, []);
const version = model.version;
await model.reloadChanged(["/r/src/a.js"]);
assert.deepEqual(listed, ["/r/src"]);
assert.equal(model.version, version, "the same listing changed no rows");
listed = [];
await model.reloadChanged(["/r/elsewhere/deep/file"]); // in no folder on screen
assert.deepEqual(listed, []);
listed = [];
await model.reloadChanged(["/r"]); // the root itself: anything may have changed
assert.deepEqual(listed.sort(), ["/r", "/r/src"]);
// Hidden is what the listing says (a dotfile, or Windows' Hidden attribute);
// revealing a file in a hidden or ignored folder shows those.
assert.equal(await model.expandTo("/r/target/out"), "/r/target/out");
assert.ok(model.showIgnored && model.rows().some((r) => r.abs === "/r/target/out"));
model.showHidden = true;
assert.ok(model.rows().some((r) => r.abs === "/r/.env"));
model.showHidden = model.showIgnored = false;

// The Files panel: shown, hidden, or another child (Changes) on top.
const files = { id: "files", type: "block", content: { mod: "herdr-nvim" } };
const stack = (active, hidden = false) => ({
  id: "root", children: [{ id: "panel", hidden, children: [{ id: "body", type: "stack", active, children: [files, { id: "changes" }] }] }],
});
assert.deepEqual(panelState(stack("files")), { stack: "body", slot: "files", panel: "panel", files: true, other: false });
assert.deepEqual(panelState(stack("changes")), { stack: "body", slot: "files", panel: "panel", files: false, other: true });
assert.equal(panelState(stack("files", true)).files, false);
// Without a stack, the slot's own `hidden` says it (hidePanel hides it).
assert.equal(panelState({ id: "root", children: [{ ...files }] }).files, true);
assert.equal(panelState({ id: "root", children: [{ ...files, hidden: true }] }).files, false);
assert.equal(panelState({ id: "root", children: [] }), null);

// Paths the tree sends.
assert.equal(path.join("/r/src", "../b/./c.txt"), "/r/b/c.txt");
assert.ok(path.inside("/r/src/a", "/r") && path.inside("/r", "/r") && !path.inside("/rx/a", "/r"));

// The controller's idea of a tab's nvim: its own open counts until the app
// lists the pane, and stops counting once the pane ended without ever being
// listed live (closed by something else meanwhile), or EXPECT_MS later.
app.spaces = [{ id: "s", tabs: [{ id: "t" }] }];
app.terminals = [];
app.exec = async () => ok(JSON.stringify({ terminal: "t9" }));
const sync = new Sync({ spawn() {}, notify() {} });
await sync.toggle({ space: "s", tab: "t", terminal: null, cwd: "/r" }, "open");
assert.equal(sync.nvimOf("t"), true, "just opened, not listed yet");
app.terminals = [{ address: "t9", state: "finished", labels: { "herdr-nvim.sidebar": "t" } }];
assert.equal(sync.nvimOf("t"), false, "ended before it was ever listed live");
app.terminals = [];
await sync.toggle({ space: "s", tab: "t", terminal: null, cwd: "/r" }, "open");
const now = Date.now;
Date.now = () => now() + 3500;
assert.equal(sync.nvimOf("t"), false, "never listed: the expectation runs out");
Date.now = now;
app.terminals = [{ address: "t9", state: "live", labels: { "herdr-nvim.sidebar": "t" } }];
app.panes = [{ pane: "p9", terminal: "t9", tab: "t" }];
assert.equal(sync.nvimOf("t"), true);
// Parked (Files closed it: running, in no pane): not open.
app.panes = [];
assert.equal(sync.nvimOf("t"), false, "a parked nvim counts as open");

// The controller's own show, not on screen yet: the next look waits for it
// (read as is, it is the person closing Files, and nvim would close).
app.focused = { space: "s", tab: "t", terminal: "t1", cwd: "/r" };
app.terminals = [{ address: "t9", state: "live", labels: { "herdr-nvim.sidebar": "t" } }];
app.panes = [{ pane: "p9", terminal: "t9", tab: "t" }];
app.layout = stack("files", true); // the panel hidden, nvim on screen
app.ran = [];
// A real wait, on a timer that doesn't keep the checks running (the
// controller's config loop sleeps between reads for good).
const cx = { spawn: (fn) => { fn(cx); }, notify() {}, sleep: (ms) => new Promise((done) => setTimeout(done, ms).unref()) };
const controller = new Sync(cx);
controller.look(cx); // nvim without its tree: show the panel
assert.deepEqual(app.ran.map(([name]) => name), ["layout.select", "layout.show"]);
controller.look(cx); // the app hasn't applied it yet
controller.look(cx);
await new Promise((done) => setTimeout(done, 20)); // whatever those looks started
app.layout = stack("files"); // now it has
controller.look(cx);
await new Promise((done) => setTimeout(done, 20));
assert.deepEqual(app.ran.map(([name]) => name), ["layout.select", "layout.show"], "nothing closed or opened meanwhile");

// The tree key (a command from nvim or the tree): the tree hides, nvim stays
// and gets the keyboard, and the tab remembers it; the next hide by Files
// closes nvim as usual.
app.layout = stack("files");
app.ran = [];
const keyed = new Sync(cx);
keyed.look(cx); // the pair on screen: nothing to do
await keyed.toggleTree(cx);
assert.deepEqual(app.ran.map(([name]) => name), ["layout.hide", "pane.focus"]);
app.layout = stack("files", true);
keyed.look(cx);
await new Promise((done) => setTimeout(done, 20));
assert.ok(!app.ran.some(([name]) => name === "pane.close"), "the tree key closed nvim");
assert.equal(keyed.tabState("t").kept, true);
app.layout = stack("files");
keyed.look(cx); // Files again: shows the tree, nvim already there
await new Promise((done) => setTimeout(done, 20));
app.layout = stack("files", true);
keyed.look(cx); // Files off: nvim closes with it
await new Promise((done) => setTimeout(done, 20));
assert.ok(app.ran.some(([name, args]) => name === "pane.close" && args.pane === "p9"), "Files off kept nvim");

// Preview: herdr-nvim swaps the preview in (app-preview); until the lists
// show it, the tab still has its nvim (else the tree would hide: a flicker).
app.exec = async (cmd, args) => ok(args[0] === "app-preview" ? JSON.stringify({ preview: "pv", closed: "t9" }) : "{}");
const swapper = new Sync(cx);
await swapper.startSwap("t", "/r/x.md", cx);
app.terminals = []; app.panes = []; // nvim's pane gone, the preview not listed yet
assert.equal(swapper.nvimOf("t"), true, "mid-swap, the tab lost its nvim");
app.panes = [{ pane: "pv", terminal: null, tab: "t", mod: "herdr-nvim" }];
assert.equal(swapper.nvimOf("t"), true);
assert.equal(swapper.tabState("t").expect, null, "the swap's expectation outlived what it waited for");

// No Neovim to start a tab's nvim with (herdr-nvim exits 3): one card for
// requests failing together, none for the tree's quiet lookup, no toast.
const { nvim } = await import("../mods/herdr-nvim/lib.js");
const { welcomeRows } = await import("../mods/herdr-nvim/card.js");
app.focused = { space: "s", tab: "t", terminal: "t1", cwd: "/r" };
app.exec = async () => ({ code: 3, stdout: JSON.stringify({ nvim_missing: { bin: "nvim", found: null, need: "0.10" } }), stderr: "" });
app.overlays = []; app.shared["herdr-nvim.card-at"] = 0;
const { openCard } = await import("../mods/herdr-nvim/card.js");
openCard({ kind: "welcome", rows: [], config: "" }); // just before: it holds nothing back
app.overlays = [];
assert.equal(await nvim("fs", ["{}"], null, { quiet: true }), null);
assert.equal(app.overlays.length, 0, "the tree's quiet lookup opened the card");
assert.equal(await nvim("toggle", ["--state", "open"]), null);
assert.equal(await nvim("open", ["/r/x.js"]), null);
assert.deepEqual(app.overlays.map((o) => o.props.card.kind), ["nvim"], "not exactly one card for two failing requests");
assert.equal(app.overlays[0].props.card.bin, "nvim");

// The welcome lists the keys keys.json binds (any section), not ones the
// install couldn't take; nvim's own explorer has no tree rows.
const keys = { global: { "cmd-e": "mod.open:herdr-nvim", "cmd-shift-e": "herdr-nvim.reveal" }, terminal: { "cmd-k o": "herdr-nvim.pick" } };
const rows = welcomeRows(keys, { tree: "native", tree_key: "<leader>e" });
assert.deepEqual(rows.map(([k]) => k), ["cmd-e", "cmd-k o", "cmd-shift-e", "<leader>e", "?"]);
assert.deepEqual(welcomeRows({ global: { "cmd-e": "mod.open:herdr-nvim" } }, { tree: "nvim", tree_key: "<leader>e" }),
  [["cmd-e", "open or close nvim"]]);

// Mermaid in the preview: fenced ```mermaid blocks split out of the text
// (not one inside another fence, nor an unclosed one), each text part with
// the document's link definitions (not footnotes, not after an unclosed
// fence). A line starting with backticks that hold more backticks is code
// in a paragraph, not a fence (else it hides every diagram after it).
const { splitMermaid } = await import("../mods/herdr-nvim/mermaid.js");
assert.deepEqual(splitMermaid("# T\n\n```mermaid\ngraph TD\n  A-->B\n```\nafter"),
  [{ md: "# T\n" }, { mermaid: "graph TD\n  A-->B" }, { md: "after" }]);
assert.deepEqual(splitMermaid("~~~ Mermaid\nx\n~~~"), [{ mermaid: "x" }]);
assert.deepEqual(splitMermaid("````md\n```mermaid\nx\n```\n````"), [{ md: "````md\n```mermaid\nx\n```\n````" }]);
assert.deepEqual(splitMermaid("a\n```mermaid\nx"), [{ md: "a\n```mermaid\nx" }]);
assert.deepEqual(splitMermaid("```js\nx\n```\n[a]: http://a"), [{ md: "```js\nx\n```\n[a]: http://a" }]);
assert.deepEqual(splitMermaid("See [a][] and [b].[^n]\n```mermaid\nx\n```\n[b] again.\n\n```txt\n[c]: not one\n```\n[a]: http://a\n[b]: http://b \"B\"\n[^n]: note"),
  [{ md: "See [a][] and [b].[^n]\n\n[a]: http://a\n[b]: http://b \"B\"" }, { mermaid: "x" },
   { md: "[b] again.\n\n```txt\n[c]: not one\n```\n[a]: http://a\n[b]: http://b \"B\"\n[^n]: note\n\n[a]: http://a\n[b]: http://b \"B\"" }]);
assert.deepEqual(splitMermaid("[a]: http://a\n```mermaid\nx\n```\n```js\nunclosed"),
  [{ md: "[a]: http://a\n\n[a]: http://a" }, { mermaid: "x" }, { md: "```js\nunclosed" }]);
assert.deepEqual(splitMermaid("```` ```mermaid ```` blocks are drawn.\n\n```mermaid\nx\n```"),
  [{ md: "```` ```mermaid ```` blocks are drawn.\n" }, { mermaid: "x" }]);
assert.deepEqual(splitMermaid("```mermaid title=\"Plan\"\nx\n```"), [{ mermaid: "x" }]);

// --- Windows, and the programs the mods run ---------------------------------

// Both mods run herdr-nvim's launcher, named one way: the header mod carries
// the same path.js (it can't import this mod's files).
const modsDir = new URL("../mods/", import.meta.url);
assert.equal(readFileSync(new URL("herdr-nvim-header/path.js", modsDir), "utf8"), readFileSync(new URL("herdr-nvim/path.js", modsDir), "utf8"),
  "herdr-nvim-header/path.js is no longer a copy of herdr-nvim/path.js: copy it again");
assert.equal(path.bin(), "/nonexistent-app-home/mods/herdr-nvim/bin/herdr-nvim");
app.platform = "windows";
app.home = "C:\\Users\\Me\\.config\\herdr-app";
assert.equal(path.bin(), "C:\\Users\\Me\\.config\\herdr-app\\mods\\herdr-nvim\\bin\\herdr-nvim.exe");

// The mods start no program but herdr-nvim and the system's opener (no git,
// which herdr-nvim runs; no sh, cat, cp or PowerShell: they differ by OS, or
// Windows lacks them).
const started = new Set();
for (const mod of ["herdr-nvim", "herdr-nvim-header"]) {
  for (const file of readdirSync(new URL(`${mod}/`, modsDir)).filter((f) => f.endsWith(".js"))) {
    const source = readFileSync(new URL(`${mod}/${file}`, modsDir), "utf8");
    for (const [, program] of source.matchAll(/(?<![.\w])exec\(\s*([^,]+?)\s*,/g)) started.add(program.trim());
  }
}
assert.deepEqual([...started].sort(), ["bin()", "opener"]);
const { systemOpen } = await import("../mods/herdr-nvim/ops.js");
const opened = [];
app.exec = async (cmd, args) => { opened.push([cmd, ...args]); return { code: 1, stdout: "", stderr: "" }; };
await systemOpen("C:/r/a b.txt");
app.platform = "linux";
await systemOpen("/r/a b.txt");
app.platform = "macos";
await systemOpen("/r/a b.txt");
assert.deepEqual(opened, [["explorer.exe", "C:\\r\\a b.txt"], ["xdg-open", "/r/a b.txt"], ["open", "/r/a b.txt"]]);

// Windows paths: every form the app, git and nvim give is one path, compared
// case aside; a drive's root is a root. Never case-folded on macOS or Linux.
app.platform = "windows";
for (const [given, canonical] of [
  ["\\\\?\\C:\\Users\\Me\\repo\\", "C:/Users/Me/repo"], ["c:\\x\\y", "C:/x/y"], ["C:/x\\y/", "C:/x/y"],
  ["C:", "C:/"], ["d:\\", "D:/"], ["\\\\?\\UNC\\host\\share\\x", "//host/share/x"], ["\\\\host\\share", "//host/share/"],
]) assert.equal(path.canonical(given), canonical, given);
assert.equal(path.dirname("C:/x"), "C:/");
assert.equal(path.dirname("C:/"), "C:/");
assert.equal(path.dirname("//host/share/x"), "//host/share/");
assert.equal(path.basename("C:/x/y.md"), "y.md");
assert.equal(path.join("C:/r/src", "..\\b\\.\\c.txt"), "C:/r/b/c.txt");
assert.equal(path.join("C:/", "../x"), "C:/x");
assert.ok(path.same("C:/Repo/x", "c:/repo/X") && path.inside("C:/REPO/src/a", "C:/repo") && !path.inside("C:/repox/a", "C:/repo"));
assert.equal(path.relative("C:/Repo/src/a.js", "C:/repo"), "src/a.js");
assert.equal(path.relative("D:/x", "D:/"), "x");
assert.equal(path.native("C:/r/a"), "C:\\r\\a");
assert.equal(path.tilde("C:/Users/Me/.config/herdr-nvim/config.toml", "c:/users/me"), "~/.config/herdr-nvim/config.toml");
assert.deepEqual(request.create("C:/r", "new\\"), { op: "create", path: "C:/r/new", folder: true });
app.platform = "macos";
assert.ok(!path.same("/r/X", "/r/x") && !path.inside("/R/a", "/r"));
assert.equal(path.canonical("C:\\x"), "C:\\x", "on macOS \\ is a character of a name");
assert.deepEqual(request.create("/r", "a\\"), { op: "create", path: "/r/a\\", folder: false });
assert.deepEqual(request.create("/r", "new/"), { op: "create", path: "/r/new", folder: true });
assert.equal(path.tilde("/Users/me/.config/herdr-nvim/config.toml", "/Users/me"), "~/.config/herdr-nvim/config.toml");

// The tree on Windows: git names the repo in its own case, nvim a file in
// what was typed; the marks, the reveal and the rows still meet.
app.platform = "windows";
const winListings = {
  "C:/r": listing(entry("Src", true), entry("x.md")),
  "C:/r/Src": listing(entry("A.js")),
};
app.exec = async (cmd, args, opts) => {
  if (cmd === path.bin() && args[0] === "app-list") return winListings[args[1]];
  return ok(JSON.stringify({ top: "c:/R", status: " M Src/A.js\0?? x.md\0" }));
};
const winModel = new TreeModel();
winModel.reset("C:/r");
await winModel.reload();
assert.equal(markOf(winModel.marks, "C:/r/Src/A.js"), "M");
assert.equal(markOf(winModel.marks, "C:/r/x.md"), "A");
assert.equal(await winModel.expandTo("c:/R/src/a.JS"), "C:/r/Src/A.js");
assert.deepEqual(winModel.rows().map((r) => r.abs), ["C:/r/Src", "C:/r/Src/A.js", "C:/r/x.md"]);

// The "Neovim is needed" card names the key that opens nvim on this OS.
const { installHint } = await import("../mods/herdr-nvim/card.js");
const hint = installHint({ global: { "alt-e": "mod.open:herdr-nvim" } });
assert.ok(hint.includes("Then press alt-e again.") && hint.includes("no admin") && !hint.includes("Homebrew"), hint);
app.platform = "macos";
assert.ok(installHint({ global: { "cmd-e": "mod.open:herdr-nvim" } }).includes("Then press cmd-e again."));
assert.ok(installHint({}).endsWith("Then open nvim again."));
app.home = "/nonexistent-app-home";

// Remote labels are local to their core; matching a tab must include its machine.
const { nvimPanes, parkedNvim } = await import("../mods/herdr-nvim/panel.js");
const { place, isRemote, machineOf, qualify, toggleRemote } = await import("../mods/herdr-nvim/lib.js");
app.spaces = [{ id: "box/s1", machine: "box", tabs: [{ id: "box/t1" }] }];
app.focused = { space: "box/s1", tab: "box/t1", terminal: "box/t7", cwd: "/remote" };
app.terminals = [
  { address: "box/t9", state: "live", labels: { "herdr-nvim.sidebar": "t1" } },
  { address: "other/t9", state: "live", labels: { "herdr-nvim.sidebar": "t1" } },
];
app.panes = [{ pane: "box/p9", terminal: "box/t9", tab: "box/t1" }];
assert.deepEqual(nvimPanes("box/t1"), ["box/t9"]);
assert.deepEqual(nvimPanes("other/t1"), []);
assert.equal(parkedNvim("other/t1"), "other/t9");
assert.deepEqual(place(), { space: "box/s1", tab: "box/t1", terminal: "box/t7", cwd: "/remote" });
assert.ok(isRemote("box/t1") && !isRemote("t1") && !isRemote(null) && !isRemote(undefined));
assert.deepEqual([machineOf("box/t1"), machineOf("t1"), machineOf(7)], ["box", null, null]);
assert.deepEqual([qualify("box", "t9"), qualify(null, "t9")], ["box/t9", "t9"]);
assert.equal(qualify(machineOf("box/t5"), "t1"), "box/t1");
// cmd-e on a remote tab: a toggle without --state, then the shown editor focused.
app.exec = async (_cmd, args) => {
  assert.equal(args[0], "toggle");
  assert.ok(!args.includes("--state"), args.join(" "));
  assert.equal(JSON.parse(args.at(-1)).tab, "box/t1");
  return ok(JSON.stringify({ terminal: "box/t9" }));
};
app.ran = [];
await toggleRemote(place(), "overlay-1");
assert.deepEqual(app.ran, [["pane.focus", { terminal: "box/t9" }]]);
app.exec = async () => ok(JSON.stringify({ closed: "box/t9" }));
app.ran = [];
await toggleRemote(place(), "overlay-1");
assert.deepEqual(app.ran, [], "a hidden editor focuses nothing");
app.exec = async (_cmd, args) => {
  assert.deepEqual(args.slice(0, 3), ["toggle", "--state", "close"]);
  assert.equal(JSON.parse(args.at(-1)).tab, "box/t1");
  return ok(JSON.stringify({ closed: "box/t9" }));
};
app.ran = [];
const remoteSync = new Sync({ spawn() {}, notify() {} });
await remoteSync.toggle(place(), "close");
assert.deepEqual(app.ran, [], "remote toggles must use the machine-aware Rust path");
await remoteSync.startSwap("box/t1", "/remote/image.png", { notify() {} });
assert.deepEqual(app.ran, [], "unsupported remote previews must not create local preview panes");

// The Files panel on a remote tab follows the local rule: shown where the
// tab's nvim (there) is, kept when Files opens it, but Files never starts
// nvim on the machine (`local`).
assert.deepEqual(wanted(tab(true, false), tab(false, false), { ...opts, local: false }), { kept: false, nvim: false });
app.terminals = [{ address: "box/t9", state: "live", labels: { "herdr-nvim.sidebar": "t1" } }];
app.panes = [{ pane: "box/p9", terminal: "box/t9", tab: "box/t1" }];
app.layout = stack("files", true);
app.exec = async (_cmd, args) => assert.fail(`no herdr-nvim ${args[0]} for a remote tab's panel`);
app.ran = [];
const remoteLook = new Sync(cx);
remoteLook.look(cx); // the remote nvim without its tree: show the panel
assert.deepEqual(app.ran.map(([name]) => name), ["layout.select", "layout.show"]);
app.layout = stack("files");
remoteLook.look(cx);
await new Promise((done) => setTimeout(done, 20));
app.terminals = [];
app.panes = [];
app.layout = stack("files", true);
app.ran = [];
const remoteFiles0 = new Sync(cx);
remoteFiles0.look(cx); // no nvim, no panel
app.layout = stack("files"); // Files pressed
remoteFiles0.look(cx);
remoteFiles0.look(cx);
await new Promise((done) => setTimeout(done, 20));
assert.deepEqual(app.ran, [], "Files on a remote tab shows the tree and starts no nvim there");

// A remote tab's tree: its machine's herdr-nvim lists and reads git there
// (src/app/companion.rs), the same output a local tree reads here, so the
// rows and marks are a local tree's.
const { RemoteError, marksFrom } = await import("../mods/herdr-nvim/tree.js");
const { POLL_MS, keepFresh, remoteToast } = await import("../mods/herdr-nvim/tree-refresh.js");
const { Tree } = await import("../mods/herdr-nvim/tree-view.js");
const { notify } = await import("./herdr-stub.mjs");
assert.deepEqual(marksFrom("/r", status), marks, "remote marks are parsed as local ones");

// Whose machine: the tab's, else its space's (src/app/remote.rs `machine_of`).
assert.equal(sourceFor({ space: "s1", tab: "t1" }).remote, false, "a local tab is read here");
assert.equal(sourceFor({ space: "box/s1", tab: "box/t1" }).machine, "box");
assert.equal(sourceFor({ space: "box/s1", tab: null }).machine, "box", "a machine space with no tab is on its machine");
assert.equal(sourceFor({ space: "box/s1", tab: "t1" }).machine, "box", "an unprefixed tab of a machine space is on its machine");
assert.equal(sourceFor({ space: "s1", tab: "lab/t1" }).machine, "lab", "the tab's machine first");
// What a remote tree can't do here says why; a local one can do it all.
assert.deepEqual(Object.values(sourceFor({ space: "s1", tab: "t1" }).cannot), [false, false, false]);
assert.ok(Object.values(sourceFor({ space: "box/s1", tab: "box/t1" }).cannot).every((why) => typeof why === "string"));

// Every failure to read a machine is a RemoteError, never no files or no marks.
const remoteCtx = { space: "box/s1", tab: "box/t1", terminal: null, cwd: "/remote" };
const broken = sourceFor(remoteCtx);
app.exec = async () => ok("not json");
await assert.rejects(broken.list("/remote"), (e) => e instanceof RemoteError && e.machine === "box" && /not an answer: not json/.test(e.message));
await assert.rejects(broken.marks("/remote"), RemoteError, "a remote git status it can't read is an error");
for (const answer of ["null", "{}"]) {
  app.exec = async () => ok(answer);
  await assert.rejects(broken.list("/remote"), (e) => e instanceof RemoteError && /no entries/.test(e.message), answer);
}
app.exec = async () => { throw new Error("spawn failed"); };
await assert.rejects(broken.list("/remote"), (e) => e instanceof RemoteError && /spawn failed/.test(e.message));
app.exec = async () => ({ code: 1, stdout: "", stderr: "herdr-nvim: box: machine is unavailable\n" });
await assert.rejects(broken.list("/remote"), (e) => e instanceof RemoteError && e.message === "box: machine is unavailable" && !e.companion);

// Which listing failures fail the whole tree: the root's on a machine, or a
// machine whose herdr-nvim can't serve this build; never a subfolder gone
// meanwhile, never a local one.
const goneThere = new RemoteError("box: not a folder: /remote/src", "box");
assert.equal(fatalListing(goneThere, "/remote", "/remote"), true);
assert.equal(fatalListing(goneThere, "/remote/src", "/remote"), false);
assert.equal(fatalListing(new RemoteError("missing", "box", { problem: "missing" }), "/remote/src", "/remote"), true);
assert.equal(fatalListing(new Error("not a folder: /r"), "/r", "/r"), false);

// A new source for the same root (another machine): what the last one still
// had on its way never lands.
const late = [];
const slowSource = { remote: true, machine: "old", cannot: {}, list: () => new Promise((done) => late.push(() => done([entry("old.txt")]))), marks: () => new Promise((done) => late.push(() => done({ "/remote/old.txt": "M" }))) };
const switched = new TreeModel();
switched.reset("/remote", slowSource);
const lateWork = Promise.all([switched.load("/remote"), switched.refreshMarks()]);
switched.reset("/remote", { remote: true, machine: "box", cannot: {}, list: async () => [entry("new.txt")], marks: async () => ({}) });
await switched.reload();
late.forEach((land) => land());
await lateWork;
assert.deepEqual(switched.rows().map((r) => r.abs), ["/remote/new.txt"]);
assert.deepEqual(switched.marks, {});

const remoteFiles = { "/remote": [entry("src", true), entry("x.md")] };
let remoteCalls = [];
let companion = null;
let goneOnMachine = null;
let unreachable = false;
app.exec = async (cmd, args) => {
  assert.equal(cmd, bin);
  assert.deepEqual(args.slice(-2), ["--ctx", JSON.stringify(remoteCtx)]);
  remoteCalls.push(args.slice(0, 2));
  if (companion) return { code: 4, stdout: JSON.stringify({ companion }), stderr: `herdr-nvim: ${companion.message}\n` };
  if (unreachable) return { code: 1, stdout: "", stderr: "herdr-nvim: box: machine is unavailable\n" };
  if (args[1] === goneOnMachine) return { code: 1, stdout: "", stderr: `herdr-nvim: box: not a folder: ${goneOnMachine}\n` };
  if (args[0] === "app-git-status") return ok(JSON.stringify({ top: "/remote", status: "?? x.md\0" }));
  return ok(JSON.stringify({ entries: remoteFiles[args[1]] ?? [], truncated: false }));
};
const remoteModel = new TreeModel();
remoteModel.reset("/remote", sourceFor(remoteCtx));
await remoteModel.reload();
assert.deepEqual(remoteModel.rows().map((r) => r.abs), ["/remote/src", "/remote/x.md"]);
assert.equal(markOf(remoteModel.marks, "/remote/x.md"), "A");
assert.deepEqual(remoteCalls, [["app-list", "/remote"], ["app-git-status", "/remote"]]);
// An open folder removed there meanwhile keeps its rows, as a local one does.
remoteFiles["/remote/src"] = [entry("a.js")];
await remoteModel.toggle("/remote/src");
goneOnMachine = "/remote/src";
await remoteModel.reload();
assert.deepEqual(remoteModel.rows().map((r) => r.abs), ["/remote/src", "/remote/src/a.js", "/remote/x.md"]);
goneOnMachine = "/remote";
await assert.rejects(remoteModel.reload(), RemoteError, "a root it can't list is an error");
goneOnMachine = null;
await remoteModel.toggle("/remote/src");
const install = "cargo install --locked --root ~/.local --git https://github.com/iurimadeira/herdr-nvim --rev abc herdr-nvim";
companion = { machine: "box", problem: "missing", install, title: "herdr-nvim is not installed on box", remedy: `install it there: ${install}`, message: `herdr-nvim is not installed on box; install it there: ${install}` };
await assert.rejects(remoteModel.reload(), (e) => e instanceof RemoteError && e.companion.machine === "box");
companion = null;

// The toast is herdr-nvim's own wording (companion.rs `title`, `remedy`),
// also for a build no companion can match (no install command); any other
// failure there names the machine and says what it answered.
const dirty = { machine: "box", problem: "mismatch", install: null, title: "herdr-nvim on box is 1.0.0 (x), not 1.3.1 (y-dirty)", remedy: "this desktop build has uncommitted changes, so no companion matches it; use a build from a commit" };
assert.deepEqual(remoteToast(new RemoteError("m", "box", dirty)), { title: dirty.title, body: dirty.remedy, level: "error" });
assert.deepEqual(remoteToast(new RemoteError("box: machine is unavailable", "box")), { title: "Cannot list files on box", body: "box: machine is unavailable", level: "error" });

// Polling: every POLL_MS while the tree shows; while hidden, even right
// after the root is set, nothing is asked of the machine. A file made there
// shows on the next poll. A failure toasts once per distinct message, and
// the tree comes back once the machine answers.
const ticks = [];
const sleep = (ms) => { assert.equal(ms, POLL_MS); return new Promise((done) => ticks.push(done)); };
const settle = async () => { for (let i = 0; i < 50; i++) await Promise.resolve(); };
const tick = async () => { ticks.shift()(); await settle(); };
let shownNow = false, alive = true, errors = [], hiddenTimes = 0;
remoteCalls = [];
app.notified = [];
const polled = new TreeModel();
polled.reset("/remote", sourceFor(remoteCtx));
const loop = keepFresh({
  model: polled, alive: () => alive, shown: () => shownNow, sleep, notify,
  changes: () => assert.fail("a remote root has no folder watch"),
  redraw() {}, hidden: () => hiddenTimes++, failed: (e) => errors.push(e && e.message),
});
await settle();
assert.deepEqual(remoteCalls, [], "nothing asked while hidden, even when set");
assert.equal(hiddenTimes, 1, "set while hidden: marked to catch up");
for (let i = 0; i < 3; i++) await tick();
assert.deepEqual(remoteCalls, [], "no remote calls while hidden");
shownNow = true;
remoteFiles["/remote"] = [entry("src", true), entry("new.txt"), entry("x.md")];
await tick();
assert.deepEqual(polled.rows().map((r) => r.abs), ["/remote/src", "/remote/new.txt", "/remote/x.md"]);
assert.deepEqual(remoteCalls.map((c) => c[0]), ["app-list", "app-git-status"]);
companion = { ...dirty, install, remedy: `install it there: ${install}`, message: `${dirty.title}; install it there: ${install}` };
await tick();
await tick();
assert.equal(app.notified.length, 1, "one toast, not one per poll");
assert.deepEqual(app.notified[0], { title: dirty.title, body: `install it there: ${install}`, level: "error" });
companion = null;
await tick();
unreachable = true;
await tick();
await tick();
assert.equal(app.notified.length, 2, "an unreachable machine toasts too, once");
assert.deepEqual(app.notified[1], { title: "Cannot list files on box", body: "box: machine is unavailable", level: "error" });
unreachable = false;
await tick();
assert.deepEqual(errors, [`${dirty.title}; install it there: ${install}`, null, "box: machine is unavailable", null]);
alive = false;
await tick();
await loop;

// The tree on a machine space's tab follows the local rule: the focused
// pane's folder until it gets the keyboard, then the folder and file of the
// tab's nvim there; it never follows nvim's file meanwhile, and every
// listing goes to the machine with the tab's bare context.
app.spaces = [{ id: "box/s1", machine: "box", folder: "/remote", active: true }];
app.focused = { space: "box/s1", tab: "box/t1", pane: "box/p1", terminal: "box/t7" };
app.panes = [{ pane: "box/p1", terminal: "box/t7", tab: "box/t1", cwd: "/remote/work" }];
app.layout = stack("files");
app.notified = [];
const asked = [];
const there = { "/remote/work": [entry("w.txt")], "/remote/proj": [entry("src", true)], "/remote/proj/src": [entry("a.js")] };
app.exec = async (cmd, args) => {
  assert.equal(cmd, bin);
  asked.push([args[0], args[1], JSON.parse(args.at(-1))]);
  if (args[0] === "app-current") return ok(JSON.stringify({ file: "/remote/proj/src/a.js", watch: null, tab_root: "/remote/proj" }));
  if (args[0] === "app-git-status") return ok(JSON.stringify({ top: null, status: "" }));
  if (args[0] === "app-find") assert.fail("a remote tree never searches this machine");
  return ok(JSON.stringify({ entries: there[args[1]] ?? [], truncated: false }));
};
const treeCx = { focus_handle: () => ({ is_focused: () => false, focus() {} }), spawn: (fn) => { fn(treeCx); }, notify() {}, sleep: (ms) => new Promise((done) => setTimeout(done, ms).unref()) };
const tree = new Tree(treeCx);
tree.follow(treeCx);
await settle();
assert.equal(tree.model.root, "/remote/work");
assert.deepEqual(asked[0], ["app-list", "/remote/work", { space: "box/s1", tab: "box/t1", terminal: null, cwd: "/remote/work" }]);
assert.ok(!asked.some(([command]) => command === "app-current"), "no file following on a remote tab");
await tree.revealCurrent(treeCx, true);
await settle();
assert.equal(tree.model.root, "/remote/proj");
assert.equal(tree.current, "/remote/proj/src/a.js", "nvim's file revealed");
assert.ok(asked.some(([command, path, ctx]) => command === "app-list" && path === "/remote/proj" && ctx.cwd === "/remote/proj" && ctx.tab === "box/t1"));
// What it can't do there says so, and `/` never searches this machine.
tree.actions.find();
assert.equal(tree.filter, null);
tree.actions.systemOpen(tree.visibleRows()[0]);
tree.action("rename", tree.visibleRows()[0], treeCx);
assert.deepEqual(app.notified.map((n) => n.title), ["Not on box", "Not on box", "Not on box"]);
// Keys and clicks act on what it shows: nothing while the machine fails.
assert.ok(tree.visibleRows().length > 0);
tree.error = "box: machine is unavailable";
assert.deepEqual(tree.visibleRows(), []);
tree.close();

console.log("js checks: ok");
