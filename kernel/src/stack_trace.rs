//! Bounded frame-chain validation. The caller supplies reads from its owned stack.

/// Walk at most `LIMIT` AAPCS64 frame records, stopping before any invalid read.
/// Records must be aligned, fully inside the current stack, and strictly ascend.
/// Return addresses must point inside the kernel text interval. A truncated or
/// corrupted chain produces only its validated prefix, without scanning memory.
pub fn walk<const LIMIT: usize>(
    mut frame: usize,
    stack: core::ops::Range<usize>,
    text: core::ops::Range<usize>,
    mut read: impl FnMut(usize) -> (usize, usize),
    mut emit: impl FnMut(usize),
) -> usize {
    let mut count = 0;
    while count < LIMIT {
        if frame < stack.start
            || !frame.is_multiple_of(16)
            || frame.checked_add(16).is_none_or(|end| end > stack.end)
        {
            break;
        }
        let (next, address) = read(frame);
        if !text.contains(&address) || !address.is_multiple_of(4) {
            break;
        }
        emit(address);
        count += 1;
        if next <= frame {
            break;
        }
        frame = next;
    }
    count
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::walk;
    #[test]
    fn stops_before_reading_guards_misalignment_cycles_and_overflow() {
        for frame in [0xff0, 0x1001, 0x1100, usize::MAX - 15] {
            assert_eq!(
                walk::<16>(
                    frame,
                    0x1000..0x1100,
                    0x4000..0x5000,
                    |_| panic!("invalid frame read"),
                    |_| panic!("invalid trace")
                ),
                0
            );
        }
        let mut reads = 0;
        assert_eq!(
            walk::<16>(
                0x1000,
                0x1000..0x1100,
                0x4000..0x5000,
                |frame| {
                    reads += 1;
                    (frame, 0x4010)
                },
                |_| ()
            ),
            1
        );
        assert_eq!(reads, 1);
    }
    #[test]
    fn bounded_valid_prefix_never_follows_bad_return_addresses() {
        let mut output = std::vec::Vec::new();
        assert_eq!(
            walk::<2>(
                0x1000,
                0x1000..0x1100,
                0x4000..0x5000,
                |frame| (frame + 16, 0x4000 + frame - 0x1000),
                |pc| output.push(pc)
            ),
            2
        );
        assert_eq!(output, [0x4000, 0x4010]);
        for pc in [0x3ffc, 0x4001, 0x5000] {
            assert_eq!(
                walk::<16>(
                    0x1000,
                    0x1000..0x1100,
                    0x4000..0x5000,
                    |_| (0x1010, pc),
                    |_| panic!("bad return address")
                ),
                0
            );
        }
    }
}
