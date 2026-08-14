const PRECISION_BITS: u32 = 12;
const M: u32 = 1 << 12;
const L: u32 = 1 << 23;
const X_MAX_BASE: u32 = (L >> 12) << 8;

#[derive(Clone, Copy)]
#[repr(C)]
struct DivEntry {
    magic32: u32,
    shift: u8,
    m_sub: u16,
}

const fn div_magic(d: u32) -> DivEntry {
    let l = 31 - d.leading_zeros();
    let pow = 1u128 << (33 + l);
    let m = ((pow + d as u128 - 1) / d as u128) as u64;
    DivEntry {
        magic32: (m - (1 << 32) - 1) as u32,
        shift: (33 + l) as u8,
        m_sub: (M - d) as u16,
    }
}

const DIV_TABLE: [DivEntry; M as usize + 1] = {
    let mut t = [DivEntry { magic32: 0, shift: 0, m_sub: 0 }; M as usize + 1];
    let mut d = 1;
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

    #[inline]
    pub fn with_capacity(n: usize) -> Self {
        Self {
            x: L,
            bytes: Vec::with_capacity(2 * n + 16),
        }
    }

    #[inline]
    pub fn put(&mut self, freq: u32, start: u32) {
        debug_assert!(freq > 0 && start + freq <= M && start < M);
        let entry = DIV_TABLE[freq as usize];
        let mut q = fast_div(self.x, entry);
        debug_assert!((q >= X_MAX_BASE) == (self.x >= X_MAX_BASE * freq));
        if q >= X_MAX_BASE {
            self.bytes.push(self.x as u8);
            self.x >>= 8;
            q >>= 8;
            if q >= X_MAX_BASE {
                self.bytes.push(self.x as u8);
                self.x >>= 8;
                q >>= 8;
            }
        }
        debug_assert!(q == self.x / freq);
        self.x = self.x + q * (entry.m_sub as u32) + start;
    }

    #[inline]
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

    #[inline]
    pub fn slot(&self) -> u32 {
        self.x & (M - 1)
    }

    #[inline]
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
fn fast_div(x: u32, d: DivEntry) -> u32 {
    let m = d.magic32 as u64 + (1u64 << 32) + 1;
    (((x as u64) * m) >> d.shift) as u32
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
    use super::{DIV_TABLE, L, M, RansDecoder, RansEncoder, X_MAX_BASE, build_decode_table, build_freq_table, fast_div};

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

    fn ref_encode(data: &[u8]) -> Vec<u8> {
        let (freq, start) = build_freq_table(data);
        let mut x = L;
        let mut bytes = Vec::new();
        for &b in data.iter().rev() {
            let f = freq[b as usize] as u32;
            let s = start[b as usize] as u32;
            let x_max = X_MAX_BASE * f;
            while x >= x_max {
                bytes.push((x & 0xff) as u8);
                x >>= 8;
            }
            let q = x / f;
            let r = x - q * f;
            x = (q << super::PRECISION_BITS) + r + s;
        }
        bytes.reverse();
        let mut out = Vec::with_capacity(4 + bytes.len());
        out.extend_from_slice(&x.to_be_bytes());
        out.extend_from_slice(&bytes);
        out
    }

    fn check_byte_identical(data: &[u8]) {
        let (freq, start) = build_freq_table(data);
        let mut enc = RansEncoder::new();
        for &b in data.iter().rev() {
            enc.put(freq[b as usize] as u32, start[b as usize] as u32);
        }
        assert_eq!(enc.finish(), ref_encode(data));
    }

    fn test_datasets() -> Vec<Vec<u8>> {
        let mut datasets = Vec::new();
        datasets.push(b"".to_vec());
        datasets.push(b"a".to_vec());
        datasets.push(b"ab".to_vec());
        datasets.push(b"hello, world!".to_vec());
        datasets.push((0u8..=255).collect::<Vec<_>>());
        let mut state: u64 = 1;
        let mut data = Vec::with_capacity(64 * 1024);
        for _ in 0..(64 * 1024) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            data.push((state & 0xff) as u8);
        }
        datasets.push(data);
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
        datasets.push(skewed_data);
        datasets.push(vec![b'x'; 100_000]);
        datasets
    }

    #[test]
    fn fast_div_correct() {
        for freq in 1u32..=M {
            let entry = DIV_TABLE[freq as usize];
            let mut x = L;
            for _ in 0..1024 {
                assert_eq!(fast_div(x, entry), x / freq, "freq={freq} x={x}");
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                x = x & 0x7FFFFFFF;
                if x < L {
                    x += L;
                }
            }
            assert_eq!(fast_div(0, entry), 0u32);
            assert_eq!(fast_div(freq, entry), 1u32);
            assert_eq!(fast_div(freq.wrapping_sub(1), entry), 0u32);
            let base = L - (L % freq);
            assert_eq!(fast_div(base, entry), base / freq);
            if base + freq < 0x8000_0000 {
                assert_eq!(fast_div(base + freq, entry), (base + freq) / freq);
            }
            assert_eq!(fast_div(0x7FFF_FFFF, entry), 0x7FFF_FFFF / freq);
        }
    }

    #[test]
    fn roundtrip_streaming() {
        for data in test_datasets() {
            check_streaming(&data);
        }
    }

    #[test]
    fn byte_identical_streaming() {
        for data in test_datasets() {
            check_byte_identical(&data);
        }
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut uniform = Vec::with_capacity(64 * 1024);
        for _ in 0..(64 * 1024) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            uniform.push((state & 0xff) as u8);
        }
        check_byte_identical(&uniform);
    }
}
