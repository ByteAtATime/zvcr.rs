fn plane_masks<const BITS: u8>() -> [u64; 8] {
    let bits = BITS as usize;
    let vpl = 64 / bits;
    let mut masks = [0u64; 8];
    for p in (0..bits).rev() {
        let mut m = 0u64;
        for i in 0..vpl {
            m |= 1u64 << (p + i * bits);
        }
        masks[p] = m;
    }
    masks
}

fn plane_mask_scalar<const BITS: u8>(packed: &[u64]) -> u8 {
    let bits = BITS as usize;
    let masks = plane_masks::<BITS>();
    let all_or = packed.iter().fold(0u64, |a, &c| a | c);
    let mut mask = 0u8;
    for p in (0..bits).rev() {
        if all_or & masks[p] != 0 {
            mask |= 1 << p;
        }
    }
    mask
}

fn pack_bitplanes_scalar<const BITS: u8, const UNPACKED_SIZE: usize>(
    packed: &[u64],
    mask: u8,
    out: &mut [u8],
) {
    let bits = BITS as usize;
    let vpl = 64 / bits;
    let bytes_per_long = vpl / 8;
    let plane_bytes = UNPACKED_SIZE / 8;
    let mut slot = 0usize;
    for p in (0..bits).rev() {
        if mask & (1 << p) == 0 {
            continue;
        }
        let plane_off = slot * plane_bytes;
        for (long_idx, &cell) in packed.iter().enumerate() {
            let mut gathered = 0u64;
            for i in 0..vpl {
                gathered |= ((cell >> (i * bits + p)) & 1) << i;
            }
            out[plane_off + long_idx * bytes_per_long..plane_off + (long_idx + 1) * bytes_per_long]
                .copy_from_slice(&gathered.to_le_bytes()[..bytes_per_long]);
        }
        slot += 1;
    }
}

fn unpack_bitplanes_scalar<const BITS: u8, const UNPACKED_SIZE: usize>(
    src: &[u8],
    mask: u8,
    packed: &mut [u64],
) {
    let bits = BITS as usize;
    let vpl = 64 / bits;
    let bytes_per_long = vpl / 8;
    let plane_bytes = UNPACKED_SIZE / 8;
    let mut slot = 0usize;
    for p in (0..bits).rev() {
        if mask & (1 << p) == 0 {
            continue;
        }
        let plane_off = slot * plane_bytes;
        for (long_idx, cell) in packed.iter_mut().enumerate() {
            let chunk = &src[plane_off + long_idx * bytes_per_long
                ..plane_off + (long_idx + 1) * bytes_per_long];
            let mut gathered = 0u64;
            for b in 0..bytes_per_long {
                gathered |= (chunk[b] as u64) << (b * 8);
            }
            for i in 0..vpl {
                *cell |= ((gathered >> i) & 1) << (i * bits + p);
            }
        }
        slot += 1;
    }
}

pub(crate) fn compute_plane_mask(packed: &[u64], bits_per_entry: u8) -> u8 {
    match bits_per_entry {
        1 => plane_mask_scalar::<1>(packed),
        2 => plane_mask_scalar::<2>(packed),
        4 => plane_mask_scalar::<4>(packed),
        8 => plane_mask_scalar::<8>(packed),
        _ => unreachable!(
            "bitplane transform only valid for indirect palettes (bpe 1/2/4/8), got {bits_per_entry}"
        ),
    }
}

pub(crate) fn pack_bitplanes_into<const UNPACKED_SIZE: usize>(
    packed: &[u64],
    bits_per_entry: u8,
    mask: u8,
    out: &mut [u8],
) {
    match bits_per_entry {
        1 => pack_bitplanes_scalar::<1, UNPACKED_SIZE>(packed, mask, out),
        2 => pack_bitplanes_scalar::<2, UNPACKED_SIZE>(packed, mask, out),
        4 => pack_bitplanes_scalar::<4, UNPACKED_SIZE>(packed, mask, out),
        8 => pack_bitplanes_scalar::<8, UNPACKED_SIZE>(packed, mask, out),
        _ => unreachable!(
            "bitplane transform only valid for indirect palettes (bpe 1/2/4/8), got {bits_per_entry}"
        ),
    }
}

pub(crate) fn unpack_bitplanes_into<const UNPACKED_SIZE: usize>(
    src: &[u8],
    bits_per_entry: u8,
    mask: u8,
    packed: &mut [u64],
) {
    match bits_per_entry {
        1 => unpack_bitplanes_scalar::<1, UNPACKED_SIZE>(src, mask, packed),
        2 => unpack_bitplanes_scalar::<2, UNPACKED_SIZE>(src, mask, packed),
        4 => unpack_bitplanes_scalar::<4, UNPACKED_SIZE>(src, mask, packed),
        8 => unpack_bitplanes_scalar::<8, UNPACKED_SIZE>(src, mask, packed),
        _ => unreachable!(
            "bitplane transform only valid for indirect palettes (bpe 1/2/4/8), got {bits_per_entry}"
        ),
    }
}
