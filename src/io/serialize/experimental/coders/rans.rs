const PRECISION_BITS: u32 = 12;
const M: u32 = 1 << 12;
const L: u32 = 1 << 23;
const X_MAX_BASE: u32 = (L >> 12) << 8;

#[derive(Clone, Copy)]
struct DivMagic {
    magic: u32,
    shift: u8,
}

const fn div_magic(d: u32) -> DivMagic {
    let l = 31 - d.leading_zeros();
    if d & (d - 1) == 0 {
        return DivMagic { magic: 0, shift: (l - 1) as u8 };
    }
    let pow = 1u128 << (33 + l);
    let magic = ((pow + d as u128 - 1) / d as u128) as u32;
    DivMagic { magic, shift: l as u8 }
}

const DIV_TABLE: [DivMagic; M as usize + 1] = {
    let mut t = [DivMagic { magic: 0, shift: 0 }; M as usize + 1];
    let mut d = 2;
    while d <= M as usize {
        t[d] = div_magic(d as u32);
        d += 1;
    }
    t
};

#[derive(Clone, Copy)]
#[repr(C)]
pub struct DecEntry {
    pub freq: u16,
    pub start: u16,
    pub sym: u8,
}

pub struct RansEncoder {
    x: u32,
    bytes: Vec<u8>,
}

impl RansEncoder {
    pub fn new() -> Self {
        Self {
            x: L,
            bytes: Vec::new(),
        }
    }

    pub fn put(&mut self, freq: u32, start: u32) {
        debug_assert!(freq > 0 && start + freq <= M && start < M);
        let x_max = X_MAX_BASE * freq;
        while self.x >= x_max {
            self.bytes.push((self.x & 0xff) as u8);
            self.x >>= 8;
        }
        let q = if freq > 1 {
            fast_div(self.x, DIV_TABLE[freq as usize])
        } else {
            self.x
        };
        let r = self.x - q * freq;
        self.x = (q << PRECISION_BITS) + r + start;
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.bytes.reverse();
        let mut out = Vec::with_capacity(4 + self.bytes.len());
        out.extend_from_slice(&self.x.to_be_bytes());
        out.extend_from_slice(&self.bytes);
        out
    }
}

pub struct RansDecoder<'a> {
    x: u32,
    body: &'a [u8],
    ip: usize,
}

impl<'a> RansDecoder<'a> {
    pub fn new(body: &'a [u8]) -> Self {
        let head = body.get(..4).expect("rans decoder body must be at least 4 bytes");
        let x = u32::from_be_bytes([head[0], head[1], head[2], head[3]]);
        Self { x, body, ip: 4 }
    }

    pub fn slot(&self) -> u32 {
        self.x & (M - 1)
    }

    pub fn advance(&mut self, freq: u32, start: u32) {
        let slot = self.x & (M - 1);
        self.x = freq * (self.x >> PRECISION_BITS) + slot - start;
        while self.x < L {
            let b = self.body.get(self.ip).copied().unwrap_or(0);
            self.ip += 1;
            self.x = (self.x << 8) | (b as u32);
        }
    }
}

#[inline(always)]
fn fast_div(x: u32, d: DivMagic) -> u32 {
    let q = ((x as u64) * (d.magic as u64) >> 32) as u32;
    let t = ((x - q) >> 1) + q;
    t >> d.shift
}

fn starts(freq: &[u16; 256]) -> [u16; 256] {
    let mut start = [0u16; 256];
    let mut acc: u32 = 0;
    for s in 0..256 {
        start[s] = acc as u16;
        acc += freq[s] as u32;
    }
    start
}

pub fn build_freq_table(data: &[u8]) -> ([u16; 256], [u16; 256]) {
    let mut counts = [0u64; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let total: u64 = counts.iter().sum();
    let mut freq = [0u16; 256];
    if total > 0 {
        for s in 0..256 {
            if counts[s] > 0 {
                let scaled = (counts[s] * (M as u64) / total) as u32;
                freq[s] = scaled.max(1) as u16;
            }
        }
        let mut sum: i64 = freq.iter().map(|&f| f as i64).sum();
        while sum < M as i64 {
            let s = (0..256)
                .filter(|&s| counts[s] > 0)
                .max_by_key(|&s| counts[s])
                .unwrap();
            freq[s] += 1;
            sum += 1;
        }
        while sum > M as i64 {
            let s = (0..256)
                .filter(|&s| freq[s] > 1)
                .max_by_key(|&s| freq[s])
                .unwrap();
            freq[s] -= 1;
            sum -= 1;
        }
        debug_assert!(freq.iter().map(|&f| f as u32).sum::<u32>() == M);
    }
    (freq, starts(&freq))
}

pub fn build_decode_table(freq: &[u16; 256], start: &[u16; 256]) -> Vec<DecEntry> {
    let mut table = vec![DecEntry { freq: 0, start: 0, sym: 0 }; M as usize];
    for s in 0..256 {
        for k in 0..(freq[s] as usize) {
            table[(start[s] as usize) + k] = DecEntry { freq: freq[s], start: start[s], sym: s as u8 };
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::{DIV_TABLE, L, M, RansDecoder, RansEncoder, build_decode_table, build_freq_table, fast_div};

    fn check_streaming(data: &[u8]) {
        let (freq, start) = build_freq_table(data);
        let mut enc = RansEncoder::new();
        for &b in data.iter().rev() {
            enc.put(freq[b as usize] as u32, start[b as usize] as u32);
        }
        let body = enc.finish();
        let table = build_decode_table(&freq, &start);
        let mut dec = RansDecoder::new(&body);
        let mut out = Vec::with_capacity(data.len());
        for _ in 0..data.len() {
            let e = unsafe { table.get_unchecked(dec.slot() as usize) };
            out.push(e.sym);
            dec.advance(e.freq as u32, e.start as u32);
        }
        assert_eq!(out, data);
    }

    #[test]
    fn fast_div_correct() {
        for freq in 2u32..=M {
            let dm = DIV_TABLE[freq as usize];
            let mut x = L;
            for _ in 0..1024 {
                assert_eq!(fast_div(x, dm), x / freq, "freq={freq} x={x}");
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                x = x & 0x7FFFFFFF;
                if x < L {
                    x += L;
                }
            }
            assert_eq!(fast_div(0, dm), 0u32);
            assert_eq!(fast_div(freq, dm), 1u32);
            assert_eq!(fast_div(freq.wrapping_sub(1), dm), 0u32);
        }
    }

    #[test]
    fn roundtrip_streaming() {
        check_streaming(b"");
        check_streaming(b"a");
        check_streaming(b"ab");
        check_streaming(b"hello, world!");
        check_streaming(&(0u8..=255).collect::<Vec<_>>());
        let mut state: u64 = 1;
        let mut data = Vec::with_capacity(64 * 1024);
        for _ in 0..(64 * 1024) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            data.push((state & 0xff) as u8);
        }
        check_streaming(&data);
        let mut skewed: u64 = 1;
        let mut skewed_data = Vec::with_capacity(128 * 1024);
        for _ in 0..(128 * 1024) {
            skewed ^= skewed << 13;
            skewed ^= skewed >> 7;
            skewed ^= skewed << 17;
            if (skewed & 7) == 0 {
                skewed_data.push((skewed & 0xff) as u8);
            } else {
                skewed_data.push(b'a');
            }
        }
        check_streaming(&skewed_data);
        check_streaming(&vec![b'x'; 100_000]);
    }
}