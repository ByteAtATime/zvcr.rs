use crate::io::serialize::error::ReadError;

pub(crate) const ESCAPE: u8 = 0x59;

const THRESHOLD: usize = 3;
const MAX_MARKER_RUN: usize = THRESHOLD + 127;

pub(crate) fn encode(src: &[u8], dst: &mut Vec<u8>) {
    let mut i = 0;
    while i < src.len() {
        let value = src[i];

        if value != 0x00 && value != 0xFF {
            if value == ESCAPE {
                dst.push(ESCAPE);
                dst.push(ESCAPE);
            } else {
                dst.push(value);
            }
            i += 1;
            continue;
        }

        let mut run_end = i + 1;
        while run_end < src.len() && src[run_end] == value {
            run_end += 1;
        }

        let run_len = run_end - i;
        if run_len < THRESHOLD {
            dst.extend(std::iter::repeat_n(value, run_len));
            i = run_end;
            continue;
        }

        let value_bit: u8 = if value == 0xFF { 0x80 } else { 0x00 };

        let mut remaining = run_len;
        while remaining >= THRESHOLD {
            let chunk = remaining.min(MAX_MARKER_RUN);
            let n = (chunk - THRESHOLD) as u8;
            let code = value_bit | n;

            if code == ESCAPE {
                let adjusted_chunk = chunk - 1;
                let adjusted_n = (adjusted_chunk - THRESHOLD) as u8;
                dst.push(ESCAPE);
                dst.push(value_bit | adjusted_n);
                remaining -= adjusted_chunk;
            } else {
                dst.push(ESCAPE);
                dst.push(code);
                remaining -= chunk;
            }
        }

        dst.extend(std::iter::repeat_n(value, remaining));
        i = run_end;
    }
}

pub(crate) fn decode(src: &[u8], dst: &mut [u8]) -> Result<(), ReadError> {
    let mut si = 0;
    let mut di = 0;

    while di < dst.len() {
        if si >= src.len() {
            return Err(ReadError::Generic(format!(
                "RLE decode underflow: produced {di}/{} output bytes",
                dst.len()
            )));
        }

        let b = src[si];
        si += 1;

        if b != ESCAPE {
            dst[di] = b;
            di += 1;
            continue;
        }

        if si >= src.len() {
            return Err(ReadError::Generic(
                "RLE decode truncated after escape byte".to_string(),
            ));
        }

        let code = src[si];
        si += 1;

        if code == ESCAPE {
            dst[di] = ESCAPE;
            di += 1;
            continue;
        }

        let value = if code & 0x80 != 0 { 0xFF } else { 0x00 };
        let count = THRESHOLD + (code & 0x7F) as usize;

        if di + count > dst.len() {
            return Err(ReadError::Generic(format!(
                "RLE decode overflow: run of {count} at output offset {di}, target {}",
                dst.len()
            )));
        }

        dst[di..di + count].fill(value);
        di += count;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(input: &[u8]) {
        let mut encoded = Vec::new();
        encode(input, &mut encoded);
        let mut decoded = vec![0u8; input.len()];
        decode(&encoded, &mut decoded).unwrap();
        assert_eq!(decoded, input, "roundtrip mismatch for input len {}", input.len());
    }

    #[test]
    fn empty_input_roundtrips() {
        roundtrip(&[]);
    }

    #[test]
    fn all_zeros_roundtrips() {
        roundtrip(&vec![0x00; 512]);
    }

    #[test]
    fn all_ones_roundtrips() {
        roundtrip(&vec![0xFF; 512]);
    }

    #[test]
    fn escape_literals_roundtrip() {
        roundtrip(&[ESCAPE, ESCAPE, ESCAPE]);
    }

    #[test]
    fn mixed_runs_and_literals_roundtrip() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&[0x00; 100]);
        buf.extend_from_slice(&[0x42, 0x43, 0x44]);
        buf.extend_from_slice(&[0xFF; 200]);
        buf.push(ESCAPE);
        buf.extend_from_slice(&[0x00; 5]);
        buf.extend_from_slice(&[0xFF; 3]);
        roundtrip(&buf);
    }

    #[test]
    fn collision_run_length_roundtrips() {
        let target_n = (ESCAPE & 0x7F) as usize;
        let collision_len = THRESHOLD + target_n;
        roundtrip(&vec![0x00u8; collision_len]);
        roundtrip(&vec![0x00u8; collision_len + 1]);
        roundtrip(&vec![0x00u8; collision_len + 130]);
    }

    #[test]
    fn long_run_splits_correctly() {
        roundtrip(&vec![0x00u8; 1000]);
        roundtrip(&vec![0xFFu8; 1000]);
    }

    #[test]
    fn random_buffers_roundtrip() {
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for _ in 0..200 {
            let len = (next() % 600) as usize;
            let mut buf = Vec::with_capacity(len);
            let mut j = 0;
            while j < len {
                let choice = next() % 10;
                let run_val = if next() & 1 == 0 { 0x00u8 } else { 0xFF };
                let run_len = (next() % 50 + 1) as usize;
                let take = run_len.min(len - j);
                if choice < 7 {
                    buf.extend(std::iter::repeat_n(run_val, take));
                } else {
                    for _ in 0..take {
                        buf.push(next() as u8);
                    }
                }
                j += take;
            }
            roundtrip(&buf);
        }
    }
}
