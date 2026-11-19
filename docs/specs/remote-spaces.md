# Herdr app features work in remote spaces

Status: approved

## Problem statement

The Herdr desktop app runs on a Mac, but spaces and agents often live on another
machine, such as the Linux host `lab`. In those remote spaces, herdr-nvim is
local-only today. The rebased upstream draft #73 opens a remote Neovim pane with
`cmd-e`. The file tree, the `cmd-o` agent-file picker, comments to the agent and
rendered previews are still refused or empty. Code on `lab` cannot be read,
navigated or annotated for an agent with the same flow as local code.

## Solution

In a space whose `machine` is a connected remote host, every herdr-nvim app
feature behaves as it does in a local space:

- `cmd-e` editor pane
- file tree
- `cmd-o` picker for Claude, Pi and Codex agents
- comments sent to the tab's agent
- markdown, mermaid and image preview

The work happens on the remote host through a companion `herdr-nvim` install on
that host. The Mac binary delegates remote work to it through herdr-core. The
fork `iurimadeira/herdr-nvim` carries this work. Nothing is sent upstream, and
the fork is synced with `upstream/main` periodically.

## User stories

1. As a user in a `lab` space, I want `cmd-e` to open that tab's Neovim on `lab`
   with herdr-nvim loaded, so that comments and file operations work there.
2. As a user in a `lab` space, I want the file tree to show the `lab` project
   with git marks, search and file operations, so that I can navigate remote code
   without a terminal.
3. As a user in a `lab` space, I want the tree to pick up files that agents
   create or change on `lab`, so that the tree is not stale.
4. As a user in a `lab` space, I want `cmd-o` to list the files that this tab's
   agent touched, so that I can review an agent's work. This must hold whichever
   router account home (`~/.claude`, `~/.claude-main`, `~/.claude-alt`, …) or
   Codex home the agent uses.
5. As a user running Codex (`aco`), locally or remotely, I want `cmd-o` to work
   as it does for Claude and Pi.
6. As a user commenting in a remote Neovim, I want my comments delivered to the
   tab's agent on `lab`.
7. As a user, I want to preview remote markdown, mermaid and images in the app.
8. As a user, I want a clear error when the companion is missing or its version
   does not match the Mac's, so that I know to install or update it.

## Behavior and acceptance

- **Editor pane:** `cmd-e` in a remote tab opens or hides that tab's Neovim on
  the remote host, as #73 does. The remote Neovim also has the companion's
  herdr-nvim plugin on its runtimepath, so plugin features work there. Hiding
  keeps buffers, undo and cursor. Closing the Herdr tab stops its remote daemon.
- **Tree listing:** the tree in a remote space lists, on the remote host, the
  folder a local tree would root at (the tab's Neovim's folder, else the
  focused pane's, else the space folder; it reveals Neovim's file when it gets
  the keyboard, without following it continuously) and shows the same entries, ordering, hidden-file rules and git
  marks as a local tree would for the same folder contents.
- **Tree refresh:** while the tree is visible, the remote listing and git marks
  are polled every 2 seconds. While it is hidden, nothing is polled. A file
  created on the remote host appears within about 2 seconds of being created
  while the tree is visible.
- **Tree search and file operations:** search, create, rename, move, copy and
  delete act on remote files. Buffers open in the remote Neovim follow
  renames and deletes as they do locally.
- **Picker in a remote tab:** `cmd-o` lists the files the tab's agent touched,
  ordered as locally. The candidate sources are the agent's session log, git
  changes and repository files, all read on the remote host. Choosing a file
  opens it in the remote Neovim.
- **Session home discovery:** the picker reads the agent process's environment
  on the remote host from `/proc/<pid>/environ`. It uses `CLAUDE_CONFIG_DIR` for
  Claude and `CODEX_HOME` for Codex. Without the variable, it uses the default
  home (`~/.claude`, `~/.codex`). An agent started by `acc` on the main account
  resolves to `~/.claude-main`.
- **Codex:** a Codex agent tab, local or remote, gets `cmd-o` candidates from its
  Codex session rollout, as Claude tabs do from Claude logs.
- **Comments:** sending comments from a remote Neovim delivers them to the
  chosen agent terminal on the remote host. The submit behavior is the same as
  for local tabs.
- **Preview:** previewing a remote markdown, mermaid or image file shows it
  rendered in the app. The file's bytes are fetched from the remote host into
  the Mac's preview cache, and the local rendering path is unchanged after that.
- **Companion missing or mismatched:** when a remote feature is used and the
  companion is absent, or its version differs from the Mac binary's, the action
  fails. The app shows a toast naming the machine and the install or update
  command. No feature degrades silently or acts on a wrong version.
- **Local spaces:** behavior in local spaces is unchanged.

## Implementation decisions

- **Companion install:** the companion is a normal `herdr-nvim` build installed
  on the remote host by the user (`cargo install` from the fork on `lab`). The
  Mac never builds or uploads a Linux binary.
- **Delegation:** the Mac binary delegates a remote tab's work as
  `hc exec --on <machine> -- herdr-nvim <app-command> …`. This covers listing,
  git marks, search, file operations, picker candidates, comments dispatch and
  preview staging. Machine-prefixed ids (`machine/<id>`) are translated to the
  remote's bare ids before delegation, using #73's `remote` module rules.
- **Version handshake:** each delegation compares the companion's version
  against the Mac binary's before doing work. The comparison uses the build
  version plus git revision, so dev builds match only themselves.
- **Remote Neovim:** the remote Neovim keeps the user's own configuration (#73)
  and gets the companion's plugin directory on its runtimepath. Plugin calls
  that need the binary resolve to the companion through `HERDR_NVIM_SELF`.
- **Tree refresh:** remote tree refresh is polling from the JS mod, because
  `watchFolder` is local-only. There is no inotify or remote watcher.
- **Preview bytes:** remote preview bytes travel through the Mac binary
  (delegated read, base64 over `hc exec`), because the mod API's
  `readTextFile`/`writeTextFile` are UTF-8 text only with a 1 MiB read cap.
- **Codex parsing:** Codex support adds a Codex agent kind and rollout parser
  to the app session code. It is shared by the local and remote paths.
- **Fork-only files:** `dev-build.yml` builds an aarch64-apple-darwin artifact
  on `workflow_dispatch` and lives only in the fork. The tracker policy lives in
  `docs/agents/issue-tracker.md`.

## Testing decisions

- **`tests/remote_cli.rs`:** delegation command shape, id translation, version
  handshake and missing-companion errors. It drives a stub `hc` (prior art: the
  existing remote_cli tests).
- **`herdr-nvim app-*` CLI on lab:** listing, git marks, search, picker
  candidates and preview staging, run as the companion would run them against
  fixture folders and fixture session logs. Prior art: `tests/sidebar_cli.rs`
  and `tests/fixtures`.
- **Session discovery:** unit tests for environ parsing and home resolution,
  with fixture `environ` files. Codex rollout parsing is tested against fixture
  rollouts.
- **Lua `tests/run.lua`:** `test_remote.lua`, `test_comments.lua`,
  `test_dispatch.lua` and `test_fs.lua` cover plugin behavior inside the remote
  Neovim (runtimepath, `HERDR_NVIM_SELF`, comment dispatch to remote terminals).
- **`herdr-app/e2e/js-checks.mjs` with `herdr-stub.mjs`:** the mod chooses
  remote paths for machine spaces. The poll runs only while the tree is visible.
  Toast on companion error.
- **`herdr-app/e2e/remote-editor.py`:** the end-to-end remote editor flow, where
  runnable on lab.
- **Manual Mac checklist:** each user-visible behavior above, checked in a real
  `lab` space after the user installs the dev-build artifact. The Herdr app
  cannot be observed from lab.

## Failure, compatibility, and rollout

- **Companion failures:** a missing companion, a version mismatch or an
  unreachable machine produces an explicit toast. No partial or stale result is
  presented as current.
- **Known test failures:** 4 daemon process-group tests already fail on lab on
  `upstream/main` and are unrelated. They are not regressions of this work.
- **Rollout:** each change lands on the fork's `main` through a PR. On the Mac,
  the user extracts the dev-build artifact's
  `herdr-nvim-app-aarch64-apple-darwin.tar.gz` and installs it with
  `herdr-nvim-app-aarch64-apple-darwin/herdr-app/install.sh --binary herdr-nvim-app-aarch64-apple-darwin/bin/herdr-nvim`
  (the binary needs the bundle's `lua/`, `plugin/` and `herdr-app/` beside it), then runs
  `chezmoi re-add ~/.config/herdr-app/keys.json ~/.config/herdr-app/layouts/iuri.json`.
  The resulting dotfiles diff is reviewed and committed. On lab, the user runs
  `cargo install` from the same fork revision.
- **Upstream sync:** `upstream/main` is merged into the fork periodically.
  Conflicts with fork-only files are resolved in the fork.

## Out of scope

- Any comment, PR or issue on `ChmaraX/herdr-nvim`.
- Automatic installation or upload of the companion to remote hosts.
- Windows remote hosts (#73 asserts a POSIX remote).
- A filesystem watcher on the remote host.
- Making Herdr's own `cmd-o` binding or the user's `iuri` layout change beyond
  what the installer already does.

## Further notes

- PR #73's commits stay authored by their original author on `feat/remote-spaces`
  (head 3d5dc02).
- The Mac app config files `keys.json` and `layouts/iuri.json` are
  chezmoi-managed in `iurimadeira-dot-files`.
- `lab` already has `hc` on PATH and Rust via rustup (`~/.cargo/bin`).
