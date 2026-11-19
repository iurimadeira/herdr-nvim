// The folder data behind the file tree: one folder's entries and the repo's
// git marks, from herdr-nvim here or, for a remote tab, on its machine.
// Paths are the mod's (path.js).

import { exec } from "herdr";
import { bin, canonical, child, dirname, key, machineOfContext } from "./path.js";

// Why a remote tab's tree can't show its machine's files: the herdr-nvim
// there is missing or another build (`companion`, src/app/companion.rs), or
// the machine didn't answer. Never shown as an empty folder.
export class RemoteError extends Error {
  constructor(message, machine, companion = null) {
    super(message);
    this.machine = machine;
    this.companion = companion;
  }
}

// `herdr-nvim <command> <path>`'s JSON answer: here, or with a remote tab's
// `ctx` (`machine`) by the herdr-nvim on its machine. Every failure there is
// a RemoteError.
async function app(command, path, ctx = null, machine = null) {
  const failed = (message, companion = null) => (machine ? new RemoteError(message, machine, companion) : new Error(message));
  let r;
  try {
    r = await exec(bin(), [command, path, ...(ctx ? ["--ctx", JSON.stringify(ctx)] : [])]);
  } catch (e) {
    throw failed(`${bin()}: ${e?.message ?? e}`);
  }
  if (r.code === 0) {
    try { return JSON.parse(r.stdout); } catch { throw failed(`${command} ${path}: not an answer: ${r.stdout.trim().slice(0, 200)}`); }
  }
  let companion = null;
  try { companion = JSON.parse(r.stdout).companion ?? null; } catch {}
  throw failed(companion?.message ?? (r.stderr.trim().replace(/^herdr-nvim: /, "") || `Cannot read ${path} (exit ${r.code})`), companion);
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

// What a tree offers for a tab (`ctx`: herdr-nvim's --ctx): where it reads
// folders (`app-list`, src/app/list.rs: [{name, dir, hidden, ignored}],
// folders first, then by name, at most 500) and git marks (`app-git-status`:
// path key -> "M" | "A" | "D" | "U", {} outside a repo), and what else it can
// do there (`can`: false, or why not). A remote tab's machine answers the
// same commands; search, file operations and the system's app work only
// here (#4).
export function sourceFor(ctx) {
  const machine = machineOfContext(ctx);
  const here = (op) => (machine ? `${op} work only on this machine's tabs for now.` : false);
  return {
    remote: Boolean(machine),
    machine,
    cannot: { find: here("Search"), fs: here("File operations"), systemOpen: here("The system's apps") },
    list: async (folder) => {
      const entries = (await app("app-list", folder, machine && ctx, machine))?.entries;
      if (Array.isArray(entries)) return entries;
      const message = `app-list ${folder}: no entries in the answer`;
      throw machine ? new RemoteError(message, machine) : new Error(message);
    },
    marks: async (root) => {
      const status = machine ? await app("app-git-status", root, ctx, machine) : await app("app-git-status", root).catch(() => null);
      return status?.top ? marksFrom(canonical(status.top), status.status) : {};
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
