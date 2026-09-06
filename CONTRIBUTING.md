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

Do not tag construction snapshots. The first tag will be the annotated `v0.1.0` tag after the
release boundary in `ROADMAP.md` is complete.
