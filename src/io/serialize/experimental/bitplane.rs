fn pack_bitplanes_scalar<const BITS: u8, const UNPACKED_SIZE: usize>(
    packed: &[u64],
    out: &mut [u8],
) {
    let bits = BITS as usize;
    let vpl = 64 / bits;
    let bytes_per_long = vpl / 8;
    let plane_bytes = UNPACKED_SIZE / 8;
    for p in 0..bits {
        let plane_off = p * plane_bytes;
        for (long_idx, &cell) in packed.iter().enumerate() {
            let mut gathered = 0u64;
            for i in 0..vpl {
                gathered |= ((cell >> (i * bits + p)) & 1) << i;
            }
            out[plane_off + long_idx * bytes_per_long..plane_off + (long_idx + 1) * bytes_per_long]
                .copy_from_slice(&gathered.to_le_bytes()[..bytes_per_long]);
        }
    }
}

fn unpack_bitplanes_scalar<const BITS: u8, const UNPACKED_SIZE: usize>(
    src: &[u8],
    packed: &mut [u64],
) {
    let bits = BITS as usize;
    let vpl = 64 / bits;
    let bytes_per_long = vpl / 8;
    let plane_bytes = UNPACKED_SIZE / 8;
    for (long_idx, cell) in packed.iter_mut().enumerate() {
        let mut value = 0u64;
        for p in 0..bits {
            let plane_off = p * plane_bytes;
            let chunk = &src[plane_off + long_idx * bytes_per_long
                ..plane_off + (long_idx + 1) * bytes_per_long];
            let mut gathered = 0u64;
            for b in 0..bytes_per_long {
                gathered |= (chunk[b] as u64) << (b * 8);
            }
            for i in 0..vpl {
                value |= ((gathered >> i) & 1) << (i * bits + p);
            }
        }
        *cell = value;
    }
}

pub(crate) fn pack_bitplanes_into<const UNPACKED_SIZE: usize>(
    packed: &[u64],
    bits_per_entry: u8,
    out: &mut [u8],
) {
    match bits_per_entry {
        1 => pack_bitplanes_scalar::<1, UNPACKED_SIZE>(packed, out),
        2 => pack_bitplanes_scalar::<2, UNPACKED_SIZE>(packed, out),
        4 => pack_bitplanes_scalar::<4, UNPACKED_SIZE>(packed, out),
        8 => pack_bitplanes_scalar::<8, UNPACKED_SIZE>(packed, out),
        _ => unreachable!(
            "bitplane transform only valid for indirect palettes (bpe 1/2/4/8), got {bits_per_entry}"
        ),
    }
}

pub(crate) fn unpack_bitplanes_into<const UNPACKED_SIZE: usize>(
    src: &[u8],
    bits_per_entry: u8,
    packed: &mut [u64],
) {
    match bits_per_entry {
        1 => unpack_bitplanes_scalar::<1, UNPACKED_SIZE>(src, packed),
        2 => unpack_bitplanes_scalar::<2, UNPACKED_SIZE>(src, packed),
        4 => unpack_bitplanes_scalar::<4, UNPACKED_SIZE>(src, packed),
        8 => unpack_bitplanes_scalar::<8, UNPACKED_SIZE>(src, packed),
        _ => unreachable!(
            "bitplane transform only valid for indirect palettes (bpe 1/2/4/8), got {bits_per_entry}"
        ),
    }
}
