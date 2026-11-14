//! Writing a bitstream most significant bit first, the way AAC lays out its syntax.

#[derive(Default)]
pub struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append the low `bits` bits of `value` (at most 32).
    pub fn put(&mut self, value: u32, bits: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        let v = value as u64 & ((1u64 << bits) - 1);
        self.acc = (self.acc << bits) | v;
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    /// Bits written so far.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.out.len() * 8 + self.n as usize
    }

    /// Pad with zero bits up to the next byte boundary.
    pub fn align(&mut self) {
        if self.n > 0 {
            self.put(0, 8 - self.n);
        }
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.align();
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_msb_first_and_pads() {
        let mut w = BitWriter::new();
        w.put(0b101, 3);
        w.put(0xFFF, 12);
        assert_eq!(w.len(), 15);
        w.put(0, 0);
        assert_eq!(w.finish(), vec![0b1011_1111, 0b1111_1110]);
    }

    #[test]
    fn handles_wide_values() {
        let mut w = BitWriter::new();
        w.put(1, 1);
        w.put(0xDEAD_BEEF, 32);
        w.put(0x7F, 7);
        assert_eq!(w.finish(), vec![0xEF, 0x56, 0xDF, 0x77, 0xFF]);
    }
}
