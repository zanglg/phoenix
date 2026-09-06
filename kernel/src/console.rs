//! Minimal byte-oriented console formatting.

use core::fmt;

/// A device capable of accepting one output byte at a time.
pub trait ByteSink {
    /// Write one byte, waiting until the device can accept it if necessary.
    fn write_byte(&mut self, byte: u8);
}

/// Text console that applies the CRLF convention expected by serial terminals.
pub struct Console<S> {
    sink: S,
    previous_was_cr: bool,
}

impl<S> Console<S> {
    /// Wrap a byte sink as a text console.
    pub const fn new(sink: S) -> Self {
        Self {
            sink,
            previous_was_cr: false,
        }
    }

    /// Borrow the underlying sink mutably.
    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }
}

impl<S: ByteSink> fmt::Write for Console<S> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            if byte == b'\n' && !self.previous_was_cr {
                self.sink.write_byte(b'\r');
            }
            self.sink.write_byte(byte);
            self.previous_was_cr = byte == b'\r';
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use core::fmt::Write;
    use std::vec::Vec;

    use super::{ByteSink, Console};

    #[derive(Default)]
    struct RecordingSink(Vec<u8>);

    impl ByteSink for RecordingSink {
        fn write_byte(&mut self, byte: u8) {
            self.0.push(byte);
        }
    }

    #[test]
    fn converts_newlines_to_serial_crlf() {
        let mut console = Console::new(RecordingSink::default());
        write!(console, "one\ntwo\r\n").expect("recording sink is infallible");

        assert_eq!(console.sink_mut().0, b"one\r\ntwo\r\n");
    }

    #[test]
    fn remembers_carriage_return_across_format_calls() {
        let mut console = Console::new(RecordingSink::default());
        write!(console, "left\r").expect("recording sink is infallible");
        write!(console, "\nright").expect("recording sink is infallible");

        assert_eq!(console.sink_mut().0, b"left\r\nright");
    }
}
