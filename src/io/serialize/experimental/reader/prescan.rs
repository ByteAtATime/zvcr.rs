use crate::io::buffer::PooledBytes;
use crate::io::serialize::error::ReadError;
use crate::io::serialize::experimental::layout::{self, BUCKETS, Domain};
use crate::io::serialize::primitives::ByteCursor;
use crate::region::palette::MAX_INDIRECT_PALETTE_SIZE;

pub(super) struct PackedCursors {
    pub(super) cursors: [ByteCursor; BUCKETS],
    pub(super) tail_start: usize,
}

pub(super) fn prescan_domain(
    probe: &mut ByteCursor,
    domain: Domain,
    paletted_count: usize,
    sizes: &mut [usize; BUCKETS],
) -> Result<(), ReadError> {
    for _ in 0..paletted_count {
        let local_len = probe.read_u16()? as usize;
        if local_len > MAX_INDIRECT_PALETTE_SIZE {
            return Err(ReadError::LengthExceeded(format!(
                "local palette length {local_len} exceeds {MAX_INDIRECT_PALETTE_SIZE}"
            )));
        }
        probe.skip(local_len * 2)?;
        let bpe = layout::palette_bpe(local_len);
        sizes[domain.bucket(bpe)] += layout::packed_byte_len(domain.cell_count(), bpe);
    }
    Ok(())
}

pub(super) fn build_packed_cursors(
    data: &PooledBytes,
    start: usize,
    sizes: &[usize; BUCKETS],
) -> Result<PackedCursors, ReadError> {
    if sizes[Domain::Biome.bucket(16)] != 0 {
        return Err(ReadError::Generic(
            "direct palette record in biome domain is unrepresentable".to_string(),
        ));
    }
    let mut starts = [0usize; BUCKETS];
    let mut end = start;
    for (bucket_start, &size) in starts.iter_mut().zip(sizes) {
        *bucket_start = end;
        end = end.saturating_add(size);
        if end > data.len() {
            return Err(ReadError::LengthExceeded(format!(
                "packed substreams end at {end} beyond body length {}",
                data.len()
            )));
        }
    }
    let cursors = std::array::from_fn(|index| {
        let mut cursor = ByteCursor::new(data.clone());
        cursor.pos = starts[index];
        cursor
    });
    Ok(PackedCursors {
        cursors,
        tail_start: end,
    })
}
