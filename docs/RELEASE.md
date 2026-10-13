# Candidate packaging and reproduction

Run the complete canonical/static and target commands in README before preparing
a candidate. `cargo xtask package` builds and inspects the minimal system and
collects these files under `target/phoenix/packages/<version>/`:

- `phoenix-system.kernel.elf`: exact kernel symbols/debug data;
- `phoenix-system.bin` and `phoenix-system.map`: bootable image and link map;
- `phoenix-system.cpio`: deterministic initramfs;
- `phoenix-system.elf`, `phoenix-child.elf`, `phoenix-fault.elf`: stripped user ELFs;
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

The supported release package is the tested debug profile. An optimized release
profile, additional machines or changed capacities need their own evidence.

After technical completion, `0.1.0-pre.N` is an annotated review candidate.
The owner must inspect and explicitly accept it before `0.1.0` promotion and the
final annotated tag. Follow CONTRIBUTING.md for the `dev` to `main` review flow.

Incremental compilation is disabled by the artifact-building commands. A first
reproduction attempt found differing incremental object suffixes in otherwise
equivalent linker maps. Disabling `CARGO_INCREMENTAL` removes that nondeterminism;
the repeat comparison includes the map rather than excluding or normalizing it.
