// The folder data behind the file tree: one folder's entries and the repo's
// git marks. Paths are the mod's (path.js).

import { exec } from "herdr";
import { bin, canonical, child, dirname, key, machineOf } from "./path.js";

// One folder's entries as the tree shows them (`herdr-nvim app-list`,
// src/app/list.rs): [{name, dir, hidden, ignored}], folders first, then by
// name, at most 500.
export async function listFolder(path) {
  const r = await exec(bin(), ["app-list", path]);
  if (r.code !== 0) throw new Error(r.stderr.trim().replace(/^herdr-nvim: /, "") || `Cannot list ${path}`);
  return JSON.parse(r.stdout).entries;
}

// A folder's repo top (null outside one).
const tops = new Map();
// Only a repo found is kept: a folder may become one (`git init`).
async function topOf(cwd) {
  if (!tops.has(cwd)) {
    const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd }).catch(() => null);
    if (top?.code !== 0) return null;
    tops.set(cwd, canonical(top.stdout.trim()));
  }
  return tops.get(cwd);
}

// Path key (path.js `key`) -> "M" | "A" | "D" | "U" for files git reports,
// {} outside a repo.
export async function gitMarks(cwd) {
  const top = await topOf(cwd);
  if (!top) return {};
  // Writes nothing under .git, so the folder watch doesn't fire again.
  const status = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=normal"], { cwd }).catch(() => null);
  return status?.code === 0 ? marksFrom(top, status.stdout) : {};
}

// The marks in `git status --porcelain=v1 -z` output, for the repo at `top`.
export function marksFrom(top, status) {
  const marks = {};
  for (const entry of status.split("\0").filter(Boolean)) {
    const xy = entry.slice(0, 2);
    // An untracked folder comes once, as `dir/`: its row gets the mark.
    marks[key(child(top, entry.slice(3).replace(/\/$/, "")))] = xy === "??" || xy.includes("A") ? "A" : xy.includes("U") ? "U" : xy.includes("D") ? "D" : "M";
  }
  return marks;
}

// Why a remote tab's tree can't show its machine's files (the herdr-nvim
// there is missing or another build: `companion`, src/app/companion.rs, or
// the machine didn't answer). Never shown as an empty folder.
export class RemoteError extends Error {
  constructor(message, companion = null) {
    super(message);
    this.companion = companion;
  }
}

async function onMachine(command, path, ctx) {
  const r = await exec(bin(), [command, path, "--ctx", JSON.stringify(ctx)]);
  if (r.code === 0) return JSON.parse(r.stdout);
  let companion = null;
  try { companion = JSON.parse(r.stdout).companion ?? null; } catch {}
  throw new RemoteError(companion?.message ?? (r.stderr.trim().replace(/^herdr-nvim: /, "") || `Cannot list ${path}`), companion);
}

// Where the tree reads a tab's folders and marks: here, or for a remote tab
// (`ctx`: herdr-nvim's --ctx) the herdr-nvim on its machine, which lists
// and reads git the same way.
export function sourceFor(ctx) {
  if (!machineOf(ctx?.tab)) return { remote: false, list: listFolder, marks: gitMarks };
  return {
    remote: true,
    list: async (folder) => (await onMachine("app-list", folder, ctx)).entries,
    marks: async (root) => {
      const { top, status } = await onMachine("app-git-status", root, ctx);
      return top ? marksFrom(canonical(top), status) : {};
    },
  };
}

// A row's mark: its own, else "A" inside a new folder (git lists that
// folder alone, not the files in it).
export function markOf(marks, abs) {
  const own = marks[key(abs)];
  if (own) return own;
  for (let dir = abs, up = dirname(abs); up !== dir; dir = up, up = dirname(up)) {
    if (marks[key(up)] === "A") return "A";
  }
  return null;
}
