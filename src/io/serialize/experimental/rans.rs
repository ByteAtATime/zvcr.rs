pub(crate) const ANS_L: u32 = 1 << 16;
pub(crate) const ANS_M: u32 = 4096;

pub(crate) const ZERO_LIMIT: u32 = 14;
pub(crate) const ZERO_DELTA: u32 = 5;
pub(crate) const EXP_LIMIT: u32 = 8;
pub(crate) const EXP_DELTA: u32 = 3;
pub(crate) const MANT_LIMIT: u32 = 8;
pub(crate) const MANT_DELTA: u32 = 3;

pub(crate) struct RansEncoder {
    state: u32,
    buf: Vec<u8>,
}

impl RansEncoder {
    pub(crate) fn new() -> Self {
        Self {
            state: ANS_L,
            buf: Vec::with_capacity(1024),
        }
    }

    #[inline]
    pub(crate) fn encode_symbol(&mut self, start: u32, freq: u32) {
        if freq == 0 {
            return;
        }
        let limit = (ANS_L / ANS_M) * 256 * freq;
        while self.state >= limit {
            self.buf.push((self.state & 0xff) as u8);
            self.state >>= 8;
        }
        self.state = (self.state / freq) * ANS_M + (self.state % freq) + start;
    }

    pub(crate) fn flush(self) -> Vec<u8> {
        let mut res = self.buf;
        let s = self.state;
        res.extend_from_slice(&s.to_le_bytes());
        res
    }
}

pub(crate) struct RansDecoder<'a> {
    state: u32,
    buf: &'a [u8],
    off: usize,
}

impl<'a> RansDecoder<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 4 {
            return Err("invalid ANS data: too short");
        }
        let state_off = data.len() - 4;
        let state = u32::from_le_bytes([
            data[state_off],
            data[state_off + 1],
            data[state_off + 2],
            data[state_off + 3],
        ]);
        Ok(Self {
            state,
            buf: &data[..state_off],
            off: state_off,
        })
    }

    #[inline]
    pub(crate) fn get_current_freq(&self) -> u32 {
        self.state % ANS_M
    }

    #[inline]
    pub(crate) fn advance(&mut self, start: u32, freq: u32) {
        if freq == 0 {
            return;
        }
        self.state = freq * (self.state / ANS_M) + (self.state % ANS_M) - start;
        while self.state < ANS_L && self.off > 0 {
            self.off -= 1;
            self.state = (self.state << 8) | self.buf[self.off] as u32;
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BitChance {
    p: u16,
    count: u8,
}

impl BitChance {
    pub(crate) const fn fresh() -> Self {
        Self {
            p: (ANS_M / 2) as u16,
            count: 0,
        }
    }

    #[inline]
    pub(crate) fn record_bit(
        &mut self,
        recs: &mut Vec<BitRec>,
        bit: u32,
        limit: u32,
        delta: u32,
    ) {
        recs.push(BitRec {
            f0: self.p as u32,
            bit: bit as u8,
        });
        self.update(bit, limit, delta);
    }

    #[inline]
    pub(crate) fn decode_bit(
        &mut self,
        dec: &mut RansDecoder,
        limit: u32,
        delta: u32,
    ) -> u32 {
        let f0 = self.p as u32;
        let slot = dec.get_current_freq();
        let bit = if slot < f0 {
            dec.advance(0, f0);
            0
        } else {
            dec.advance(f0, ANS_M - f0);
            1
        };
        self.update(bit, limit, delta);
        bit
    }

    #[inline]
    fn update(&mut self, bit: u32, limit: u32, delta: u32) {
        if (self.count as u32) < limit {
            self.count += 1;
        }
        let n = self.count as u32 + delta;
        let p = self.p as u32;
        let target = if bit == 0 { ANS_M } else { 0u32 };
        let new_p = if target >= p {
            p + (target - p) / n
        } else {
            p - (p - target) / n
        };
        self.p = new_p.clamp(1, ANS_M - 1) as u16;
    }
}

pub(crate) struct NzCoder {
    zero: BitChance,
    exp: [BitChance; 16],
    mant: [BitChance; 16],
}

impl NzCoder {
    pub(crate) fn new() -> Box<Self> {
        Box::new(Self {
            zero: BitChance::fresh(),
            exp: [BitChance::fresh(); 16],
            mant: [BitChance::fresh(); 16],
        })
    }

    pub(crate) fn reset(&mut self) {
        self.zero = BitChance::fresh();
        let f = BitChance::fresh();
        self.exp.fill(f);
        self.mant.fill(f);
    }

    pub(crate) fn encode(&mut self, recs: &mut Vec<BitRec>, v: u32) {
        if v == 0 {
            self.zero.record_bit(recs, 0, ZERO_LIMIT, ZERO_DELTA);
            return;
        }
        self.zero.record_bit(recs, 1, ZERO_LIMIT, ZERO_DELTA);
        let exp = 31 - v.leading_zeros();
        for i in 0..exp as usize {
            self.exp[i].record_bit(recs, 1, EXP_LIMIT, EXP_DELTA);
        }
        self.exp[exp as usize].record_bit(recs, 0, EXP_LIMIT, EXP_DELTA);
        for i in (0..exp as usize).rev() {
            self.mant[i].record_bit(recs, (v >> i) & 1, MANT_LIMIT, MANT_DELTA);
        }
    }

    pub(crate) fn decode(&mut self, dec: &mut RansDecoder) -> u32 {
        if self.zero.decode_bit(dec, ZERO_LIMIT, ZERO_DELTA) == 0 {
            return 0;
        }
        let mut exp: usize = 0;
        while self.exp[exp].decode_bit(dec, EXP_LIMIT, EXP_DELTA) == 1 {
            exp += 1;
        }
        let mut v: u32 = 1u32 << exp;
        for i in (0..exp).rev() {
            v |= self.mant[i].decode_bit(dec, MANT_LIMIT, MANT_DELTA) << i;
        }
        v
    }
}

pub(crate) struct BitRec {
    pub(crate) f0: u32,
    pub(crate) bit: u8,
}

pub(crate) fn flush_bit_recs(recs: &[BitRec]) -> Vec<u8> {
    let mut enc = RansEncoder::new();
    for rec in recs.iter().rev() {
        if rec.bit == 0 {
            enc.encode_symbol(0, rec.f0);
        } else {
            enc.encode_symbol(rec.f0, ANS_M - rec.f0);
        }
    }
    enc.flush()
}

pub(crate) fn normalize_counts(counts: &[u32]) -> Vec<u32> {
    let total: u64 = counts.iter().map(|&c| c as u64).sum();
    let active = counts.iter().filter(|&&c| c > 0).count();
    if active == 0 {
        return vec![0u32; counts.len()];
    }
    if active > ANS_M as usize {
        let mut temp: Vec<(usize, u32)> = counts
            .iter()
            .enumerate()
            .filter(|&(_, &c)| c > 0)
            .map(|(i, &c)| (i, c))
            .collect();
        temp.sort_by(|a, b| b.1.cmp(&a.1));
        let mut normalized = vec![0u32; counts.len()];
        for &(idx, _) in temp.iter().take(ANS_M as usize) {
            normalized[idx] = 1;
        }
        return normalized;
    }
    let mut normalized = vec![0u32; counts.len()];
    let mut sum: u32 = 0;
    for (idx, &c) in counts.iter().enumerate() {
        if c == 0 {
            continue;
        }
        let val = ((c as u64 * (ANS_M - active as u32) as u64) / total) as u32;
        normalized[idx] = val + 1;
        sum += normalized[idx];
    }
    let diff = ANS_M as i32 - sum as i32;
    if diff > 0 {
        let (max_idx, _) = counts
            .iter()
            .enumerate()
            .max_by_key(|&(_, &c)| c)
            .unwrap_or((0, &0u32));
        normalized[max_idx] += diff as u32;
    } else if diff < 0 {
        let mut diff = diff;
        while diff < 0 {
            let max_idx = normalized
                .iter()
                .zip(counts.iter())
                .enumerate()
                .filter(|&(_, (n, c))| *n > 1 && *c > 0)
                .max_by_key(|&(_, (_, c))| *c)
                .map(|(i, _)| i);
            match max_idx {
                Some(idx) if normalized[idx] > 1 => {
                    normalized[idx] -= 1;
                    diff += 1;
                }
                _ => break,
            }
        }
    }
    normalized
}

pub(crate) fn build_cum_freqs(freqs: &[u32]) -> Vec<u32> {
    let mut cum = Vec::with_capacity(freqs.len() + 1);
    let mut sum = 0u32;
    for &f in freqs {
        cum.push(sum);
        sum += f;
    }
    cum.push(sum);
    cum
}

pub(crate) fn build_slot_table(cum: &[u32]) -> Vec<u16> {
    let mut slots = vec![0u16; ANS_M as usize];
    for sym in 0..cum.len().saturating_sub(1) {
        for s in cum[sym]..cum[sym + 1] {
            slots[s as usize] = sym as u16;
        }
    }
    slots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rans_roundtrip_binary_bits() {
        let bits = [0u32, 1, 0, 0, 1, 1, 0, 1, 0, 0, 0, 1, 1, 1, 0, 0];
        let recs: Vec<BitRec> = bits
            .iter()
            .map(|&b| BitRec { f0: 3000, bit: b as u8 })
            .collect();
        let bytes = flush_bit_recs(&recs);

        let mut dec = RansDecoder::new(&bytes).unwrap();
        for &expected in &bits {
            let slot = dec.get_current_freq();
            let decoded = if slot < 3000 {
                dec.advance(0, 3000);
                0
            } else {
                dec.advance(3000, ANS_M - 3000);
                1
            };
            assert_eq!(decoded, expected);
        }
    }

    #[test]
    fn rans_roundtrip_multi_symbol() {
        let cum = [0u32, 1000, 3000];
        let freq = [1000u32, 2000, 1096];
        let symbols = [0u32, 1, 2, 0, 1, 1, 0, 2, 1, 0, 0, 1, 2, 1, 0];

        let mut enc = RansEncoder::new();
        for &s in symbols.iter().rev() {
            enc.encode_symbol(cum[s as usize], freq[s as usize]);
        }
        let bytes = enc.flush();

        let mut dec = RansDecoder::new(&bytes).unwrap();
        for &expected in &symbols {
            let slot = dec.get_current_freq();
            let mut decoded = 0;
            for i in (0..3).rev() {
                if slot >= cum[i] {
                    decoded = i as u32;
                    break;
                }
            }
            dec.advance(cum[decoded as usize], freq[decoded as usize]);
            assert_eq!(decoded, expected);
        }
    }
}
