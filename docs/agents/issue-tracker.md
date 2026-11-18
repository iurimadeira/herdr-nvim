# Issue tracker

- Ownership: personal.
- Tracker: GitHub Issues on the fork `iurimadeira/herdr-nvim` (`gh issue ... -R iurimadeira/herdr-nvim`).
  Never create issues, PRs or comments on `ChmaraX/herdr-nvim`.
- PRs target `iurimadeira/herdr-nvim` `main`. External PRs are not a request surface.
- Dependencies: native GitHub sub-issues for parent links and native "blocked by"
  when available; otherwise a `## Blocked by` list in the body.

## Status mapping

Phases follow idea-to-ship's Ticket lifecycle.

| Phase | GitHub mechanics |
| --- | --- |
| started | issue open + label `started` |
| in review | PR linked in a comment; label `in-review` |
| ready to integrate | not mapped (stays `in-review`) |
| merged | comment with merge commit; label `merged`; stays open |
| done | closed as completed, comment naming verification evidence |
| canceled | closed as not planned, comment with reason |

PRs reference issues without closing keywords. Repository auto-close on merge is
GitHub's keyword-only behavior, so omitting keywords keeps issues open.
