const RANGE_BITS: u32 = 12;
const TOP_BYTE: u32 = 0xff00_0000;

#[inline]
fn midpoint(low: u32, high: u32, prob: u32) -> u32 {
    let prob = prob.clamp(1, 4094) as u64;
    let range = (high - low) as u64;
    low + ((range * prob) >> RANGE_BITS) as u32
}

#[inline]
fn shares_top_byte(low: u32, high: u32) -> bool {
    (low ^ high) & TOP_BYTE == 0
}

pub(crate) struct Encoder {
    low: u32,
    high: u32,
    out: Vec<u8>,
}

impl Default for Encoder {
    fn default() -> Self {
        Self {
            low: 0,
            high: u32::MAX,
            out: Vec::new(),
        }
    }
}

impl Encoder {
    #[inline]
    pub(crate) fn encode(&mut self, prob: u32, truth: u32) {
        let mid = midpoint(self.low, self.high, prob);
        if truth != 0 {
            self.high = mid;
        } else {
            self.low = mid + 1;
        }
        while shares_top_byte(self.low, self.high) {
            self.out.push((self.high >> 24) as u8);
            self.low <<= 8;
            self.high = (self.high << 8) | 255;
        }
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        for _ in 0..4 {
            self.out.push((self.low >> 24) as u8);
            self.low <<= 8;
        }
        self.out
    }
}

pub(crate) struct Decoder<'a> {
    low: u32,
    high: u32,
    code: u32,
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        let mut code = 0u32;
        for i in 0..4 {
            code = (code << 8) | *data.get(i).unwrap_or(&0) as u32;
        }
        Self {
            low: 0,
            high: u32::MAX,
            code,
            data,
            pos: 4,
        }
    }

    #[inline]
    fn next_byte(&mut self) -> u32 {
        let byte = *self.data.get(self.pos).unwrap_or(&255) as u32;
        self.pos += 1;
        byte
    }

    #[inline]
    pub(crate) fn decode(&mut self, prob: u32) -> u32 {
        let mid = midpoint(self.low, self.high, prob);
        let bit = (self.code <= mid) as u32;
        if bit != 0 {
            self.high = mid;
        } else {
            self.low = mid + 1;
        }
        while shares_top_byte(self.low, self.high) {
            self.low <<= 8;
            self.high = (self.high << 8) | 255;
            self.code = (self.code << 8) | self.next_byte();
        }
        bit
    }
}
