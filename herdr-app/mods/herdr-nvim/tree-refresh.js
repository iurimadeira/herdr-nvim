// Keeps a tree's listing current while the tree shows. A local root lists
// again what its folder watch reports; a remote one has no watch, so it
// lists everything again every POLL_MS. A tree left hidden lists nothing for
// changes (`hidden`) and catches up once when shown, a local one in the
// view's render. A remote one asks its machine nothing while hidden, not
// even when set, and catches up on its next poll, which the view wakes early
// by ending `sleep`.

import { GIT_EVERY_MS } from "./tree-model.js";
import { RemoteError } from "./tree.js";

export const POLL_MS = 2000;

// `model`: a TreeModel at its root. `changes()`: the local folder watch's next
// batch. `failed(error | null)` and `notify` run only when the error changes,
// so a machine that can't list toasts once, not every poll, and the tree comes
// back once it can.
export async function keepFresh({ model, alive, shown, changes, sleep, redraw, hidden, failed, notify }) {
  let last = null, marksDue = false;
  const attempt = async (work) => {
    let error = null, result = null;
    try { result = await work(); } catch (e) { error = e; }
    if (!alive()) return null;
    const message = error ? String(error.message ?? error) : null;
    if (message !== last) {
      last = message;
      failed(error);
      if (error instanceof RemoteError) notify(remoteToast(error));
    }
    return result;
  };
  if (shown() || !model.source.remote) await attempt(() => model.reload());
  else hidden();
  redraw();
  while (alive()) {
    const changed = model.source.remote ? await sleep(POLL_MS).then(() => null) : await changes();
    if (!alive()) return;
    if (!shown()) {
      hidden();
      continue;
    }
    const result = await attempt(() => model.reloadChanged(changed));
    if (result?.marks === "none") continue;
    redraw();
    if (result?.marks === "later" && !marksDue) {
      marksDue = true;
      sleep(GIT_EVERY_MS).then(async () => {
        marksDue = false;
        if (!alive()) return;
        await attempt(() => model.refreshMarks());
        redraw();
      });
    }
  }
}

// Why a remote tree shows no files: the machine's herdr-nvim and how to fix
// it, in herdr-nvim's own words (src/app/companion.rs), else the machine's
// answer.
export function remoteToast(error) {
  const c = error.companion;
  if (c) return { title: c.title, body: c.remedy, level: "error" };
  return { title: `Cannot list files on ${error.machine}`, body: error.message, level: "error" };
}
