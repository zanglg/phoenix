# Candidate packaging and reproduction

Commit the candidate source and require a clean worktree. Run the complete
canonical/static and target commands in README before preparing
a candidate. `cargo xtask package` builds and inspects the minimal system and
collects these files under `target/phoenix/packages/<version>/`:

- `phoenix-system.kernel.elf`: exact kernel symbols/debug data;
- `phoenix-system.bin` and `phoenix-system.map`: bootable image and link map;
- `phoenix-system.cpio`: deterministic initramfs;
- `phoenix-system.elf`, `phoenix-child.elf`, `phoenix-fault.elf`: stripped user ELFs;
- `phoenix-cooperative.elf`: development scheduling conformance ELF (added for 0.2);
- `PROVENANCE.txt`: full source commit, dirty/clean state, checkout path, compiler,
  Cargo, available QEMU identity, target/profile and board configuration;
- `SHA256SUMS`: hashes of every preceding file.

`sha256sum` is required for packaging. Inside that directory, run
`sha256sum --check SHA256SUMS`. Keep the exact ELF when interpreting a panic trace;
a later build may assign different addresses even when a function name is unchanged.

Reproduction is defined for the same source state, tool versions, debug profile,
checkout path and build options. Debug ELF and linker-map paths are not normalized,
so arbitrary checkout paths are not promised to be byte-identical. There are no
wall-clock fields in the archive or generated identity. The source dirty flag and
commit are intentionally part of the kernel identity; clean and dirty images differ.

The validation report compares two builds after deleting only the generated
system kernel and userspace target directories between them. It compares kernel
ELF/raw image/map, deterministic archive and all three user ELFs. Provenance and
hash files preserve the tested state; packaging itself is not a runtime test.
Packaging rejects dirty source before building and checks that the full commit and
worktree remain unchanged before copying artifacts. The candidate tag must point
to that tested commit. Runtime evidence is retained outside the source commit so
adding a transcript does not change the tag/package identity. GitHub CI uploads
logs and the package under `phoenix-runtime-<source SHA>` for 30 days; preserve a
local copy for longer retention. A documentation follow-up is a new source identity
and must not silently replace the tagged package.

The supported release package is the tested debug profile. An optimized release
profile, additional machines or changed capacities need their own evidence.

After technical completion, `0.1.0-pre.N` is an annotated review candidate.
The owner must inspect and explicitly accept it before `0.1.0` promotion and the
final annotated tag. Follow CONTRIBUTING.md for the `dev` to `main` review flow.

Incremental compilation is disabled by the artifact-building commands. A first
reproduction attempt found differing incremental object suffixes in otherwise
equivalent linker maps. Disabling `CARGO_INCREMENTAL` removes that nondeterminism;
the repeat comparison includes the map rather than excluding or normalizing it.

## Accepted 0.1.0 release on dev

On 2026-10-15 the owner accepted 0.1.0-pre.2 and instructed continued development
on dev. This release promotes that implementation on dev, runs all formal-source
checks there and creates annotated v0.1.0 on the exact tested dev commit. PR #1
remains the dev-to-main review record; this release does not require its merge.
The usual merge/recreate procedure applies only when a main merge is requested.

Formal artifacts are regenerated at version 0.1.0; the pre.2 package remains
historical. The formal receipt records the release commit, clean worktree, owner
acceptance, canonical/static/MSRV results, all 24 QEMU outcomes, seven-artifact
reproduction and hashes, GitHub CI and tag identity. Evidence remains outside the
source commit so writing it does not change the tested identity. Retain the exact
ELF and provenance with the package; later dev commits are not the released source.
