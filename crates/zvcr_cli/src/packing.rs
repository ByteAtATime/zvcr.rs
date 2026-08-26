pub fn bits_per_index(palette_len: usize, palette_min_bits: u8) -> u32 {
    let adjusted = std::cmp::max(palette_len, 1) as u64 - 1;
    let width = if adjusted == 0 {
        0
    } else {
        64 - adjusted.leading_zeros()
    };
    std::cmp::max(width, palette_min_bits as u32)
}

pub fn pack_section<const N: usize>(
    unpacked: &[u16; N],
    section_size: usize,
    palette_min_bits: u8,
) -> (Vec<u16>, Vec<i64>) {
    let mut palette: Vec<u16> = Vec::new();
    let mut indices: std::collections::HashMap<u16, u16> = std::collections::HashMap::new();
    for &value in unpacked.iter().take(section_size) {
        if let std::collections::hash_map::Entry::Vacant(slot) = indices.entry(value) {
            slot.insert(palette.len() as u16);
            palette.push(value);
        }
    }
    if palette.len() == 1 {
        return (palette, Vec::new());
    }
    let bits = bits_per_index(palette.len(), palette_min_bits);
    let entries_per_long = 64 / bits as usize;
    let packed_size = section_size.div_ceil(entries_per_long);
    let mut data = vec![0i64; packed_size];
    let mask = (1u16 << bits) - 1;
    for i in 0..section_size {
        let long_index = i / entries_per_long;
        let index_in_long = i % entries_per_long;
        let shift = index_in_long * bits as usize;
        let palette_index = indices[&unpacked[i]];
        let packed_value = (palette_index & mask) as i64;
        let clear_mask = !(((1i64 << bits) - 1) << shift);
        data[long_index] &= clear_mask;
        data[long_index] |= packed_value << shift;
    }
    (palette, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack<const N: usize>(
        data: &[i64],
        palette: &[u16],
        section_size: usize,
        min_bits: u8,
    ) -> [u16; N] {
        let bits = bits_per_index(palette.len(), min_bits) as usize;
        let entries = 64 / bits;
        let mut out = [0u16; N];
        for i in 0..section_size {
            let long_index = i / entries;
            let index_in_long = i % entries;
            let shift = index_in_long * bits;
            let raw = (data[long_index] >> shift) & ((1i64 << bits) - 1);
            out[i] = palette[raw as usize];
        }
        out
    }

    #[test]
    fn test_single_value_empty_data() {
        let mut arr = [0u16; 4096];
        arr.fill(7);
        let (palette, data) = pack_section(&arr, 4096, 4);
        assert_eq!(palette, vec![7]);
        assert!(data.is_empty());
    }

    #[test]
    fn test_bits_for_17() {
        assert_eq!(bits_per_index(17, 4), 5);
    }

    #[test]
    fn test_block_roundtrip() {
        let mut arr = [0u16; 4096];
        for i in 0..4096 {
            arr[i] = (i % 17) as u16;
        }
        let (palette, data) = pack_section(&arr, 4096, 4);
        assert_eq!(palette.len(), 17);
        assert_eq!(bits_per_index(17, 4), 5);
        assert!(!data.is_empty());
        let unpacked = unpack::<4096>(&data, &palette, 4096, 4);
        assert_eq!(unpacked, arr);
    }

    #[test]
    fn test_biome_roundtrip() {
        let mut arr = [0u16; 64];
        for i in 0..64 {
            arr[i] = (i % 5) as u16;
        }
        let (palette, data) = pack_section(&arr, 64, 0);
        assert_eq!(palette.len(), 5);
        assert_eq!(bits_per_index(5, 0), 3);
        let unpacked = unpack::<64>(&data, &palette, 64, 0);
        assert_eq!(unpacked, arr);
    }
}
