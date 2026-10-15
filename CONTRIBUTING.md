# Contributing to Phoenix

Development happens on `dev`; `main` is the review boundary. Start with `AGENTS.md`, then work
only on the first incomplete section of `ROADMAP.md`. Unscheduled ideas belong in `FEATURES.md`.

Before proposing a change, run:

```bash
cargo xtask ci
```

Open a pull request from `dev` to `main`. The repository uses squash-only merging so `main`
remains linear and each accepted change has one reviewed commit. After a squash merge, recreate
`dev` from the new `main` before continuing; this avoids permanent divergence between the
integration and protected branches.

This is appropriate for Phoenix's early, mostly single-maintainer phase. If subsystem
maintainers or patch series later make individual commit authorship important, record and adopt
a rebase or maintainer-merge policy instead of silently changing history rules.

Do not tag construction snapshots. After the technical release boundary is complete, reviewed
candidates use annotated `v0.1.0-pre.N` tags. The annotated `v0.1.0` tag is created only after
explicit owner acceptance of a candidate and any requested corrections.

## Current owner-directed release flow

The owner accepted the 0.1.0-pre.2 candidate on 2026-10-15 and requested continued
work on dev. The first formal 0.1.0 release is therefore validated and tagged on
dev. Keep development on dev and PR #1 available for main review. A main merge
and any subsequent dev recreation require a separate instruction; the generic
squash workflow above does not authorize them for this release.
