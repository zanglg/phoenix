# 0.1.0-pre.1 validation report

The technical implementation passed canonical CI (232 kernel tests and 16 host-tool
checks), Rust 1.95 target/host tests, all five focused QEMU commands, thirteen
runtime probe boots and one minimal-system boot. Seven packaged artifacts matched
byte-for-byte after clean rebuilding with incremental compilation disabled.

This candidate is being recorded from the verified construction tree. The exact
clean candidate commit, retained transcripts and provenance are added in the
subsequent evidence-only commit; no candidate tag is issued before that recording.
Owner acceptance of the final 0.1.0 release remains pending.
