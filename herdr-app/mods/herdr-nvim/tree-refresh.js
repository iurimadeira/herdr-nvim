// Keeps a tree's listing current. A local root lists again what its folder
// watch reports; a remote one has no watch, so it lists everything again
// every POLL_MS while the tree shows, and asks its machine nothing while
// hidden. Either way a hidden tree catches up once when shown (`hidden`).

import { GIT_EVERY_MS } from "./tree-model.js";

export const POLL_MS = 2000;

// `model`: a TreeModel at its root. `changes()`: the local folder watch's next
// batch. `failed(error | null)` and `notify` run only when the error changes,
// so a remote machine without herdr-nvim toasts once, not every poll, and the
// tree comes back once it is installed.
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
      if (error?.companion) notify(companionToast(error.companion));
    }
    return result;
  };
  await attempt(() => model.reload());
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

// The toast for a remote machine's herdr-nvim (src/app/companion.rs): the
// machine, and the command that installs this build there.
export function companionToast({ machine, problem, install }) {
  const title = problem === "missing" ? `herdr-nvim is not installed on ${machine}` : `herdr-nvim on ${machine} is another build`;
  return { title, body: `Install this build there:\n${install}`, level: "error" };
}
