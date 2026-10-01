# Technical debt baseline

Captured with [`debtmap`](https://crates.io/crates/debtmap) v0.24.1 on
2026-10-01, at commit `c945ac0` (v0.14.1), with git-history context enabled.

```sh
debtmap analyze . --context-providers git_history
```

## Headline numbers

| Metric | Value |
|---|---|
| Total debt score | **1428** |
| Debt density | **146.2** per 1K LOC |
| Total LOC (as measured) | 9,772 |

Distribution: 5 critical, 1 high, 4 moderate.

**This is a baseline, not a scorecard.** The number is only useful
alongside the next one — re-run after a change and compare. A number that
never moves stops being information.

## Full ranking, our own code only

(`static/datastar.js` omitted. Of the top 26 findings, 11 are in that one
vendored file, including six of the eight highest.)

| # | Score | Location | Tier |
|---|---|---|---|
| 3 | 101 | `xtask/src/e2e.rs:454` `json_field()` | CRITICAL |
| 7 | 68.1 | `xtask/src/e2e.rs:589` `drive_inner()` | HIGH |
| 8 | 61.5 | `src/routes/upload.rs:54` `post_upload()` | HIGH |
| 9 | 59.7 | `xtask/src/release.rs:19` `check()` | HIGH |
| 11 | 52.8 | `xtask/src/e2e.rs:172` `http_get()` | HIGH |
| 12 | 51.4 | `src/posts.rs:38` `parse_hashtags()` | HIGH |
| 13 | 49.5 | `xtask/src/e2e.rs:674` `drain_events()` | MEDIUM |
| 15 | 48.8 | `src/bin/backfill_media_keys.rs:163` `main()` | MEDIUM |
| 18 | 32.4 | `xtask/src/e2e.rs:34` `run()` | MEDIUM |
| 19 | 29.6 | `src/routes/auth.rs:352` `post_login()` | LOW |
| 22 | 24.1 | `xtask/src/devdb.rs:50` `run()` | LOW |
| 23 | 23.9 | `xtask/src/e2e.rs:390` `WebSocket::read_frame()` | LOW |
| 24 | 21.4 | `src/database/postgres.rs:207` `create_post_with_media()` | LOW |
| 25 | 18.7 | `src/routes/auth.rs:192` `post_register()` | LOW |

Six of the fourteen are in `xtask/src/e2e.rs`. That is the concentration,
and it is the finding: a dependency-free reimplementation of three
protocols (HTTP, WebSocket, JSON) in ~900 lines. Each piece is defensible;
the question worth asking is whether the trade was worth it. `xtask` must
not be able to break `cargo check` at the repo root, which is a real
constraint — but it has also cost more debugging time than a `serde_json`
dependency would have.

Worth a human read, in order:

1. **`json_field`** — the only CRITICAL item in our code. Already caused two
   real bugs: truncated nested JSON at the first `}` (which looked like a
   valid short reply and was very hard to diagnose) and missing brace
   balancing until a regression test pinned it. 8 unit tests exist as a
   result.
2. **`parse_hashtags`** — application code on the upload path for every
   post, worth confirming the rules are the ones actually intended.
3. **`post_upload`** — largest handler, most business rules.

`create_post_with_media` and `post_register` are low tier with low
entropy-adjusted factors (0.33, 0.57), meaning the tooling considers them
well-structured. Leave them alone.

Tracked in [#5](https://github.com/crustyrustacean/halation/issues/5) and
[#6](https://github.com/crustyrustacean/halation/issues/6).

## Methodological notes

- **Six of the eight highest-scoring items are `static/datastar.js`** — the
  vendored Datastar bundle, minified to single-letter names, 115 functions in
  one file, `cyclomatic=579` at file scope.

  **Do not refactor this file.** It is vendored third-party code: generated
  output, so any hand edit is lost on the next re-vendor; no tests, because
  we did not write it; and its complexity is inherent to what it is rather
  than a defect we introduced. When the inert-controls bug shipped to
  production, the fix belonged in `templates/base.html`, not here.

  It dominates the headline score and is the least actionable thing in the
  repository. A `.debtmapignore` entry would cut the score sharply for
  reasons that have nothing to do with our own quality — so if one is added,
  record the before and after deliberately, so a much-lower number is not
  later mistaken for real improvement.

- **`--context-providers git_history`** is on deliberately. It makes the
  risk component reflect how often code actually changes, which is the part
  that correlates with real defects.
- **`debtmap analyze` does not emit machine-readable output to stdout.** The
  `--output json` flag drives an interactive TUI, and redirecting produces an
  empty file, so a byte-comparable stored baseline is not currently practical
  with v0.24.1. The numbers above are committed as markdown for that reason.
  Worth revisiting on upgrade.
- **Dead-code warnings are false positives here.** The tool reports "no
  callers detected — may be dead code" for `create_post_with_media` and
  several Datastar internals. Those are called through `async_trait` boxes
  and the Actix route registry, which the static call graph cannot see.
  Recorded here so a future run does not get "fixed".
