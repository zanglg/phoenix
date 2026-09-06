# Device Tree Support

Phoenix contains a borrowed, allocation-free parser for version-17-compatible flattened device
trees. Its behavior is Host Tested and Cross Compiled; the DTB supplied by QEMU has not yet been
read at runtime.

## Validated representation

The parser validates the fixed header, declared block ranges, required block alignments, memory
reservation termination and overlap, structure grammar, exact end token, node/property UTF-8,
property-name offsets, and property payload bounds. Property values remain borrowed byte slices
until explicitly decoded.

`BootInfo` also exposes the validated total blob size. The boot memory-map builder combines that
size with the physical boot argument so every page touched by the still-borrowed DTB is reserved.

The strings block intentionally has no alignment requirement, as specified by DTSpec. The
reservation and structure blocks require 8-byte and 4-byte offsets respectively.

## Boot information currently extracted

- boot CPU physical ID;
- `/chosen/bootargs` as a single zero-terminated UTF-8 string;
- root `#address-cells` and `#size-cells`, supporting one or two 32-bit cells;
- one or more `reg` entries from root-level `memory` nodes;
- fixed memory reservation entries.
- static `reg` entries from direct `/reserved-memory` children.

Memory ranges are returned as checked half-open physical ranges. Unsupported cell widths,
partial `reg` entries, address overflow, and a tree without non-empty memory are rejected.
Dynamic `size`-only reserved-memory requests and non-empty address-translating `ranges` are rejected
rather than silently made allocatable.

## Deferred target integration

The bootstrap currently forwards the DTB physical address but does not dereference it. Before
runtime integration, Phoenix must define how the blob's physical pages remain reserved and how
long borrowed references remain valid. Device-specific extraction for interrupt controllers,
timers, PSCI, and VirtIO will be added only when their consumers exist.
