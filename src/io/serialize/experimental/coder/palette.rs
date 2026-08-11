use super::super::rans::*;
use crate::io::serialize::primitives::{put_u16_le, put_u32_le};

const MODE_RAW: u8 = 0;
const MODE_RANS1: u8 = 2;
const MAX_ALPHABET: usize = 4000;
const ORDER1_CONTEXTS: usize = 128;

fn markov_context_for(rank: usize) -> usize {
    rank.min(ORDER1_CONTEXTS - 1)
}

fn encode_atoms_order1(entry_atoms: &[Vec<u16>], val_to_rank: &[u16]) -> Vec<u8> {
    let mut coders: Vec<Box<NzCoder>> = (0..ORDER1_CONTEXTS + 1).map(|_| NzCoder::new()).collect();
    let mut recs: Vec<BitRec> = Vec::new();
    for atoms in entry_atoms {
        let mut prev = ORDER1_CONTEXTS;
        for &atom in atoms {
            let rank = val_to_rank[atom as usize] as u32;
            coders[prev].encode(&mut recs, rank);
            prev = markov_context_for(rank as usize);
        }
    }
    flush_bit_recs(&recs)
}

fn decode_atoms_order1(
    atom_bytes: &[u8],
    lengths: &[usize],
    dict: &[u16],
) -> Result<Vec<Vec<u16>>, String> {
    let mut dec = RansDecoder::new(atom_bytes).map_err(|e| e.to_string())?;
    let mut coders: Vec<Box<NzCoder>> = (0..ORDER1_CONTEXTS + 1).map(|_| NzCoder::new()).collect();
    let mut out = Vec::with_capacity(lengths.len());
    for &n in lengths {
        let mut atoms = vec![0u16; n];
        let mut prev = ORDER1_CONTEXTS;
        for atom in atoms.iter_mut() {
            let rank = coders[prev].decode(&mut dec) as usize;
            if rank >= dict.len() {
                return Err(format!(
                    "palette order-1 rank {rank} out of dict range {}",
                    dict.len()
                ));
            }
            *atom = dict[rank];
            prev = markov_context_for(rank);
        }
        out.push(atoms);
    }
    Ok(out)
}

pub(crate) fn encode_palette_table(entries: &[Vec<u16>]) -> Vec<u8> {
    let mut buf = Vec::new();
    put_u32_le(&mut buf, entries.len() as u32);

    if entries.is_empty() {
        return buf;
    }

    let mut atom_counts = vec![0u32; 65536];
    let mut length_set = std::collections::BTreeSet::new();
    for atoms in entries {
        length_set.insert(atoms.len() as u16);
        for &atom in atoms {
            atom_counts[atom as usize] += 1;
        }
    }

    let active_atoms = atom_counts.iter().filter(|&&c| c > 0).count();
    if active_atoms > MAX_ALPHABET || length_set.len() > MAX_ALPHABET {
        buf.push(MODE_RAW);
        for atoms in entries {
            put_u16_le(&mut buf, atoms.len() as u16);
            for &atom in atoms {
                put_u16_le(&mut buf, atom);
            }
        }
        return buf;
    }

    buf.push(MODE_RANS1);

    let mut ranked_atoms: Vec<(u16, u32)> = atom_counts
        .iter()
        .enumerate()
        .filter(|&(_, &c)| c > 0)
        .map(|(i, &c)| (i as u16, c))
        .collect();
    ranked_atoms.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let dict_size = ranked_atoms.len();
    let mut val_to_rank = vec![0u16; 65536];
    put_u16_le(&mut buf, dict_size as u16);
    for (rank, &(atom, _)) in ranked_atoms.iter().enumerate() {
        val_to_rank[atom as usize] = rank as u16;
        put_u16_le(&mut buf, atom);
    }

    let length_values: Vec<u16> = length_set.iter().copied().collect();
    let num_lengths = length_values.len();
    let mut length_to_symbol = vec![0u16; 65536];
    put_u16_le(&mut buf, num_lengths as u16);
    for (symbol, &length) in length_values.iter().enumerate() {
        length_to_symbol[length as usize] = symbol as u16;
        put_u16_le(&mut buf, length);
    }

    let mut length_counts = vec![0u32; num_lengths];
    for atoms in entries {
        length_counts[length_to_symbol[atoms.len()] as usize] += 1;
    }
    let length_freqs = normalize_counts(&length_counts);
    for &freq in &length_freqs {
        put_u16_le(&mut buf, freq as u16);
    }

    let length_cum = build_cum_freqs(&length_freqs);
    let mut length_encoder = RansEncoder::new();
    for atoms in entries.iter().rev() {
        let symbol = length_to_symbol[atoms.len()] as usize;
        length_encoder.encode_symbol(length_cum[symbol], length_freqs[symbol]);
    }
    let length_bytes = length_encoder.flush();
    put_u32_le(&mut buf, length_bytes.len() as u32);
    buf.extend_from_slice(&length_bytes);

    let atom_bytes = encode_atoms_order1(entries, &val_to_rank);
    put_u32_le(&mut buf, atom_bytes.len() as u32);
    buf.extend_from_slice(&atom_bytes);

    buf
}

pub(crate) fn decode_palette_table(data: &[u8]) -> Result<Vec<Vec<u16>>, String> {
    if data.len() < 4 {
        return Err("palette: unexpected end of data".to_string());
    }
    let num_entries = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let mut pos = 4;

    if num_entries == 0 {
        return Ok(Vec::new());
    }

    if pos >= data.len() {
        return Err("palette: unexpected end of data reading mode".to_string());
    }
    let mode = data[pos];
    pos += 1;

    if mode == MODE_RAW {
        let mut entries = Vec::with_capacity(num_entries);
        for _ in 0..num_entries {
            let n = read_u16(data, &mut pos)? as usize;
            let mut atoms = vec![0u16; n];
            for atom in atoms.iter_mut() {
                *atom = read_u16(data, &mut pos)?;
            }
            entries.push(atoms);
        }
        return Ok(entries);
    }

    if mode != MODE_RANS1 {
        return Err(format!("palette: unknown mode {mode}"));
    }

    let dict_size = read_u16(data, &mut pos)? as usize;
    let mut dict = vec![0u16; dict_size];
    for entry in dict.iter_mut() {
        *entry = read_u16(data, &mut pos)?;
    }

    let num_lengths = read_u16(data, &mut pos)? as usize;
    let mut length_values = vec![0u16; num_lengths];
    for val in length_values.iter_mut() {
        *val = read_u16(data, &mut pos)?;
    }
    let mut length_freqs = vec![0u32; num_lengths];
    for freq in length_freqs.iter_mut() {
        *freq = read_u16(data, &mut pos)? as u32;
    }
    let length_cum = build_cum_freqs(&length_freqs);
    let length_slot = build_slot_table(&length_cum);

    let length_bytes_len = read_u32(data, &mut pos)? as usize;
    let length_bytes = read_slice(data, &mut pos, length_bytes_len)?;

    let atom_bytes_len = read_u32(data, &mut pos)? as usize;
    let atom_bytes = read_slice(data, &mut pos, atom_bytes_len)?;

    let mut length_decoder = RansDecoder::new(length_bytes).map_err(|e| e.to_string())?;
    let mut lengths = vec![0usize; num_entries];
    for length in lengths.iter_mut() {
        let slot = length_decoder.get_current_freq();
        let symbol = length_slot[slot as usize] as usize;
        length_decoder.advance(length_cum[symbol], length_freqs[symbol]);
        *length = length_values[symbol] as usize;
    }

    decode_atoms_order1(atom_bytes, &lengths, &dict)
}

fn read_u16(data: &[u8], pos: &mut usize) -> Result<u16, String> {
    if *pos + 2 > data.len() {
        return Err("palette: unexpected end of data".to_string());
    }
    let v = u16::from_le_bytes([data[*pos], data[*pos + 1]]);
    *pos += 2;
    Ok(v)
}

fn read_u32(data: &[u8], pos: &mut usize) -> Result<u32, String> {
    if *pos + 4 > data.len() {
        return Err("palette: unexpected end of data".to_string());
    }
    let v = u32::from_le_bytes([
        data[*pos],
        data[*pos + 1],
        data[*pos + 2],
        data[*pos + 3],
    ]);
    *pos += 4;
    Ok(v)
}

fn read_slice<'a>(data: &'a [u8], pos: &mut usize, len: usize) -> Result<&'a [u8], String> {
    if *pos + len > data.len() {
        return Err("palette: stream truncated".to_string());
    }
    let slice = &data[*pos..*pos + len];
    *pos += len;
    Ok(slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_roundtrip_simple() {
        let entries = vec![
            vec![10, 20, 30],
            vec![10, 20, 40],
            vec![10, 50],
            vec![60, 70, 80, 90],
        ];
        let encoded = encode_palette_table(&entries);
        let decoded = decode_palette_table(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn palette_roundtrip_empty() {
        let entries: Vec<Vec<u16>> = vec![];
        let encoded = encode_palette_table(&entries);
        let decoded = decode_palette_table(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }

    #[test]
    fn palette_roundtrip_large_alphabet() {
        let mut entries = Vec::new();
        for i in 0..100 {
            entries.push((0..5000).map(|j| (i * 100 + j) as u16).collect());
        }
        let encoded = encode_palette_table(&entries);
        let decoded = decode_palette_table(&encoded).unwrap();
        assert_eq!(decoded, entries);
    }
}
