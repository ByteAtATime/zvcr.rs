use crate::definitions::{SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::io::buffer::PooledBytes;
use crate::io::serialize::experimental::models::error::ModelError;
use crate::io::serialize::experimental::models::predictor::{
    CHAIN_ORDER, CHAIN_SLOTS, CHAIN_TABLE_MASK, HeadLite, HeadState, MAX_BIT_DEPTH, NONE,
    PRIMARY_TABLE_MASK, POINTER_BANDS, Predictor, SectionMetadata, TREE_BAND_TABLE_MASK,
    ADAPT_RATE_SHIFT, adapt, adapt_weights, combine, gather_neighbors, gather_neighbors_fast,
    mix_logits,
};
use crate::io::serialize::experimental::models::range::{Decoder, Encoder};
use crate::io::serialize::experimental::models::spatial::{
    SECTION_SIDE, SEGMENT_SIDE, SIDE, SectionPos, Y_STRIDE, Z_STRIDE, fill_section_lut,
    fill_section_mapped, fill_uniform, section_origin,
};
use crate::io::serialize::experimental::pack::{extract_indices, hist_indices};
use crate::io::serialize::primitives::{
    ByteCursor, put_bytes, put_u8, put_u16_le, put_u32_le, put_u64_le,
};
use crate::raw::RegionData;
use crate::region::packed_data::Data;
use crate::region::palette::ATOM_COUNT;

const MODE: u8 = 1;

fn bit_depth(distinct_len: usize) -> usize {
    (usize::BITS - (distinct_len - 1).leading_zeros()) as usize
}

struct ChainScratch {
    candidates: [u16; CHAIN_SLOTS],
    slots: [u8; CHAIN_SLOTS],
}

impl ChainScratch {
    fn new() -> Self {
        Self {
            candidates: [NONE; CHAIN_SLOTS],
            slots: [0; CHAIN_SLOTS],
        }
    }
}

struct Modeler {
    predictor: Predictor,
    inverse: Box<[u16]>,
    miss: Vec<u8>,
}

impl Modeler {
    fn new(len: usize) -> Self {
        Self {
            predictor: Predictor::new(),
            inverse: vec![0u16; ATOM_COUNT].into_boxed_slice(),
            miss: vec![0u8; len],
        }
    }

    fn encode_section(
        &mut self,
        encoder: &mut Encoder,
        voxels: &mut [u16],
        pos: SectionPos,
        palette: &[u16],
        palette_bits: usize,
    ) -> Result<(), ModelError> {
        for (i, &atom) in palette.iter().enumerate() {
            self.inverse[atom as usize] = i as u16;
        }
        let (origin_x, origin_z, origin_y) = pos.origin();
        let mut chain = ChainScratch::new();
        let mut state = HeadState::new();
        let section = SectionMetadata {
            section_y: pos.section_y,
            palette_bits,
        };
        for local_y in 0..SECTION_SIDE {
            let y = origin_y + local_y;
            for local_z in 0..SECTION_SIDE {
                let z = origin_z + local_z;
                let row = y * Y_STRIDE + z * Z_STRIDE + origin_x;
                let fast_row =
                    (pos.section_y > 0 || local_y >= 2) && (origin_z > 0 || local_z >= 2);
                for local_x in 0..SECTION_SIDE {
                    let x = origin_x + local_x;
                    let idx = row + local_x;
                    if fast_row && local_x >= 2 {
                        let east_causal =
                            x + 1 < SIDE && (local_x + 1 < SECTION_SIDE || local_y == 0);
                        gather_neighbors_fast(voxels, idx, east_causal, &mut state.neighbors);
                    } else {
                        gather_neighbors(voxels, idx, x, y, z, &mut state.neighbors);
                    }
                    let truth = voxels[idx];
                    let head = self.predictor.encode_head(
                        encoder,
                        section,
                        &mut state,
                        truth,
                        &mut self.miss,
                        idx,
                    );
                    if head.bit == 0 {
                        self.encode_residual(
                            encoder,
                            truth,
                            ResidualCtx {
                                neighbors: &state.neighbors,
                                head: &head,
                                chain: &mut chain,
                                palette,
                                palette_bits,
                                section_y: pos.section_y,
                            },
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn decode_section(
        &mut self,
        decoder: &mut Decoder,
        voxels: &mut [u16],
        pos: SectionPos,
        palette: &[u16],
        palette_bits: usize,
    ) -> Result<(), ModelError> {
        let (origin_x, origin_z, origin_y) = pos.origin();
        let mut chain = ChainScratch::new();
        let mut state = HeadState::new();
        let section = SectionMetadata {
            section_y: pos.section_y,
            palette_bits,
        };
        for local_y in 0..SECTION_SIDE {
            let y = origin_y + local_y;
            for local_z in 0..SECTION_SIDE {
                let z = origin_z + local_z;
                let row = y * Y_STRIDE + z * Z_STRIDE + origin_x;
                let fast_row =
                    (pos.section_y > 0 || local_y >= 2) && (origin_z > 0 || local_z >= 2);
                for local_x in 0..SECTION_SIDE {
                    let x = origin_x + local_x;
                    let idx = row + local_x;
                    if fast_row && local_x >= 2 {
                        let east_causal =
                            x + 1 < SIDE && (local_x + 1 < SECTION_SIDE || local_y == 0);
                        gather_neighbors_fast(voxels, idx, east_causal, &mut state.neighbors);
                    } else {
                        gather_neighbors(voxels, idx, x, y, z, &mut state.neighbors);
                    }
                    let head = self.predictor.decode_head(
                        decoder,
                        section,
                        &mut state,
                        &mut self.miss,
                        idx,
                    );
                    let value = if head.bit != 0 {
                        head.primary_value
                    } else {
                        self.decode_residual(
                            decoder,
                            ResidualCtx {
                                neighbors: &state.neighbors,
                                head: &head,
                                chain: &mut chain,
                                palette,
                                palette_bits,
                                section_y: pos.section_y,
                            },
                        )?
                    };
                    voxels[idx] = value;
                }
            }
        }
        Ok(())
    }

    fn encode_residual(
        &mut self,
        encoder: &mut Encoder,
        truth: u16,
        ctx: ResidualCtx<'_>,
    ) -> Result<u16, ModelError> {
        let mut candidate_count = 0usize;
        for &slot in CHAIN_ORDER.iter() {
            let candidate = ctx.neighbors[slot];
            if candidate == NONE || candidate == ctx.head.primary_value {
                continue;
            }
            if ctx.chain.candidates[..candidate_count].contains(&candidate) {
                continue;
            }
            ctx.chain.candidates[candidate_count] = candidate;
            ctx.chain.slots[candidate_count] = slot as u8;
            candidate_count += 1;
        }
        if candidate_count == 1 {
            let candidate = ctx.chain.candidates[0];
            let slot = ctx.chain.slots[0] as usize;
            let pointer_band = match slot {
                0..=2 => 0,
                3..=6 => 1,
                7..=8 => 2,
                _ => 3,
            };
            let pointer_ctx = ctx.palette_bits * POINTER_BANDS + pointer_band;
            let prob = self.predictor.pointer[pointer_ctx] as u32;
            let bit = (truth == candidate) as u32;
            encoder.encode(prob, bit);
            adapt(
                &mut self.predictor.pointer[pointer_ctx],
                &mut self.predictor.pointer_counts[pointer_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            if bit != 0 {
                return Ok(candidate);
            }
        } else {
            for entry in 0..candidate_count {
                let candidate = ctx.chain.candidates[entry];
                let slot = ctx.chain.slots[entry] as usize;
                let chain_ctx = (combine(
                    candidate as u32,
                    entry as u32 | (ctx.head.mask << 8),
                ) & CHAIN_TABLE_MASK) as usize;
                let prob = self.predictor.chain[slot][chain_ctx] as u32;
                let bit = (truth == candidate) as u32;
                encoder.encode(prob, bit);
                adapt(
                    &mut self.predictor.chain[slot][chain_ctx],
                    &mut self.predictor.chain_counts[slot][chain_ctx],
                    bit,
                    ADAPT_RATE_SHIFT,
                );
                if bit != 0 {
                    return Ok(candidate);
                }
            }
        }
        self.encode_tree(
            encoder,
            truth,
            ctx.head.hash,
            ctx.palette,
            ctx.palette_bits,
            ctx.section_y,
        )
    }

    fn decode_residual(
        &mut self,
        decoder: &mut Decoder,
        ctx: ResidualCtx<'_>,
    ) -> Result<u16, ModelError> {
        let mut candidate_count = 0usize;
        for &slot in CHAIN_ORDER.iter() {
            let candidate = ctx.neighbors[slot];
            if candidate == NONE || candidate == ctx.head.primary_value {
                continue;
            }
            if ctx.chain.candidates[..candidate_count].contains(&candidate) {
                continue;
            }
            ctx.chain.candidates[candidate_count] = candidate;
            ctx.chain.slots[candidate_count] = slot as u8;
            candidate_count += 1;
        }
        if candidate_count == 1 {
            let candidate = ctx.chain.candidates[0];
            let slot = ctx.chain.slots[0] as usize;
            let pointer_band = match slot {
                0..=2 => 0,
                3..=6 => 1,
                7..=8 => 2,
                _ => 3,
            };
            let pointer_ctx = ctx.palette_bits * POINTER_BANDS + pointer_band;
            let prob = self.predictor.pointer[pointer_ctx] as u32;
            let bit = decoder.decode(prob);
            adapt(
                &mut self.predictor.pointer[pointer_ctx],
                &mut self.predictor.pointer_counts[pointer_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            if bit != 0 {
                return Ok(candidate);
            }
        } else {
            for entry in 0..candidate_count {
                let candidate = ctx.chain.candidates[entry];
                let slot = ctx.chain.slots[entry] as usize;
                let chain_ctx = (combine(
                    candidate as u32,
                    entry as u32 | (ctx.head.mask << 8),
                ) & CHAIN_TABLE_MASK) as usize;
                let prob = self.predictor.chain[slot][chain_ctx] as u32;
                let bit = decoder.decode(prob);
                adapt(
                    &mut self.predictor.chain[slot][chain_ctx],
                    &mut self.predictor.chain_counts[slot][chain_ctx],
                    bit,
                    ADAPT_RATE_SHIFT,
                );
                if bit != 0 {
                    return Ok(candidate);
                }
            }
        }
        self.decode_tree(
            decoder,
            ctx.head.hash,
            ctx.palette,
            ctx.palette_bits,
            ctx.section_y,
        )
    }

    fn encode_tree(
        &mut self,
        encoder: &mut Encoder,
        truth: u16,
        hash: u32,
        palette: &[u16],
        palette_bits: usize,
        section_y: usize,
    ) -> Result<u16, ModelError> {
        let truth_index = self.inverse[truth as usize] as usize;
        let mut partial = 0usize;
        for bit_pos in (0..palette_bits).rev() {
            let node = partial | (1 << bit_pos);
            let spatial_tree_ctx = (combine(hash, node as u32) & PRIMARY_TABLE_MASK) as usize;
            let bitpos_tree_ctx = node | (bit_pos << 13);
            let height_tree_ctx =
                (combine(section_y as u32, node as u32) & TREE_BAND_TABLE_MASK) as usize;
            let probs = [
                self.predictor.tree[0][spatial_tree_ctx] as u32,
                self.predictor.tree[1][bitpos_tree_ctx] as u32,
                self.predictor.tree_band[height_tree_ctx] as u32,
            ];
            let mixed = mix_logits(&self.predictor.tree_weights[palette_bits], &probs);
            let bit = ((truth_index >> bit_pos) & 1) as u32;
            encoder.encode(mixed, bit);
            adapt(
                &mut self.predictor.tree[0][spatial_tree_ctx],
                &mut self.predictor.tree_counts[0][spatial_tree_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            adapt(
                &mut self.predictor.tree[1][bitpos_tree_ctx],
                &mut self.predictor.tree_counts[1][bitpos_tree_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            adapt(
                &mut self.predictor.tree_band[height_tree_ctx],
                &mut self.predictor.tree_band_counts[height_tree_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            adapt_weights(
                &mut self.predictor.tree_weights[palette_bits],
                &probs,
                bit,
                mixed,
            );
            partial |= (bit as usize) << bit_pos;
        }
        palette
            .get(partial)
            .copied()
            .ok_or(ModelError::PaletteIndexOutOfRange {
                index: partial,
                len: palette.len(),
            })
    }

    fn decode_tree(
        &mut self,
        decoder: &mut Decoder,
        hash: u32,
        palette: &[u16],
        palette_bits: usize,
        section_y: usize,
    ) -> Result<u16, ModelError> {
        let mut partial = 0usize;
        for bit_pos in (0..palette_bits).rev() {
            let node = partial | (1 << bit_pos);
            let spatial_tree_ctx = (combine(hash, node as u32) & PRIMARY_TABLE_MASK) as usize;
            let bitpos_tree_ctx = node | (bit_pos << 13);
            let height_tree_ctx =
                (combine(section_y as u32, node as u32) & TREE_BAND_TABLE_MASK) as usize;
            let probs = [
                self.predictor.tree[0][spatial_tree_ctx] as u32,
                self.predictor.tree[1][bitpos_tree_ctx] as u32,
                self.predictor.tree_band[height_tree_ctx] as u32,
            ];
            let mixed = mix_logits(&self.predictor.tree_weights[palette_bits], &probs);
            let bit = decoder.decode(mixed);
            adapt(
                &mut self.predictor.tree[0][spatial_tree_ctx],
                &mut self.predictor.tree_counts[0][spatial_tree_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            adapt(
                &mut self.predictor.tree[1][bitpos_tree_ctx],
                &mut self.predictor.tree_counts[1][bitpos_tree_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            adapt(
                &mut self.predictor.tree_band[height_tree_ctx],
                &mut self.predictor.tree_band_counts[height_tree_ctx],
                bit,
                ADAPT_RATE_SHIFT,
            );
            adapt_weights(
                &mut self.predictor.tree_weights[palette_bits],
                &probs,
                bit,
                mixed,
            );
            partial |= (bit as usize) << bit_pos;
        }
        palette
            .get(partial)
            .copied()
            .ok_or(ModelError::PaletteIndexOutOfRange {
                index: partial,
                len: palette.len(),
            })
    }
}

struct Scratch {
    idx_hist: Box<[u16; 256]>,
    lut: Box<[u16; 256]>,
    seen: Box<[u8; ATOM_COUNT]>,
    indices: Box<[u8; SECTION_SIZE_BLOCKS]>,
    seen_gen: u8,
    distinct: Box<[u16; SECTION_SIZE_BLOCKS]>,
}

impl Scratch {
    fn new() -> Self {
        Self {
            idx_hist: Box::new([0; 256]),
            lut: Box::new([0; 256]),
            seen: Box::new([0; ATOM_COUNT]),
            indices: Box::new([0; SECTION_SIZE_BLOCKS]),
            seen_gen: 0,
            distinct: Box::new([0; SECTION_SIZE_BLOCKS]),
        }
    }

    fn next_generation(&mut self) -> u8 {
        self.seen_gen = self.seen_gen.wrapping_add(1);
        if self.seen_gen == 0 {
            self.seen.fill(0);
            self.seen_gen = 1;
        }
        self.seen_gen
    }
}

struct SideStreams {
    headers: Vec<u8>,
    uniforms: Vec<u8>,
    palettes: Vec<u8>,
}

pub(crate) fn encode_region(
    data: &RegionData,
    section_count: usize,
) -> Result<Vec<u8>, ModelError> {
    let total_cells = SIDE * SIDE * section_count * SECTION_SIDE;
    let mut hist = vec![0u32; ATOM_COUNT];
    let mut filled_cells = 0usize;
    let mut scratch = Scratch::new();
    count_section_atoms(data, &mut hist, &mut filled_cells, &mut scratch);
    hist[0] += (total_cells - filled_cells) as u32;
    let (ranked, rank) = build_ranking(&hist);

    let voxels = build_grid(data, section_count, &rank, &mut scratch);
    encode_grid(voxels, section_count, data, &ranked, &rank, &mut scratch)
}

fn build_ranking(hist: &[u32]) -> (Vec<u16>, Vec<u16>) {
    let mut ranked: Vec<u16> = (0..ATOM_COUNT)
        .filter(|&atom| hist[atom] != 0)
        .map(|atom| atom as u16)
        .collect();
    ranked.sort_unstable_by_key(|&atom| (std::cmp::Reverse(hist[atom as usize]), atom));
    let mut rank = vec![u16::MAX; ATOM_COUNT];
    for (r, &atom) in ranked.iter().enumerate() {
        rank[atom as usize] = r as u16;
    }
    (ranked, rank)
}

fn usable_span(bits: usize, bytes: &[u8]) -> (&[u8], usize) {
    let usable = (bytes.len() * 8 / bits).min(SECTION_SIZE_BLOCKS);
    (&bytes[..usable * bits / 8], usable)
}

fn count_section_atoms(
    data: &RegionData,
    hist: &mut [u32],
    filled_cells: &mut usize,
    scratch: &mut Scratch,
) {
    for segment in data.segments.iter().flatten() {
        for section in &segment.block_sections {
            let Some(snapshot) = section.snapshots().first() else {
                continue;
            };
            *filled_cells += SECTION_SIZE_BLOCKS;
            match &snapshot.data.data {
                Data::Single(atom) => hist[*atom as usize] += SECTION_SIZE_BLOCKS as u32,
                Data::Paletted(paletted) => {
                    let bytes = &paletted.packed_long_array[..];
                    let usable;
                    if paletted.palette.direct() {
                        usable = (bytes.len() / 2).min(SECTION_SIZE_BLOCKS);
                        for pair in bytes.chunks_exact(2).take(usable) {
                            let atom = u16::from_le_bytes([pair[0], pair[1]]) as usize;
                            hist[atom] += 1;
                        }
                    } else {
                        let bits = paletted.palette.bits_per_entry;
                        let (packed, cells) = usable_span(bits, bytes);
                        usable = cells;
                        let idx_hist = &mut *scratch.idx_hist;
                        idx_hist.fill(0);
                        hist_indices(bits, packed, idx_hist);
                        let palette = &paletted.palette.palette;
                        for (index, &count) in idx_hist[..palette.len()].iter().enumerate() {
                            if count != 0 {
                                hist[palette[index] as usize] += count as u32;
                            }
                        }
                    }
                    hist[0] += (SECTION_SIZE_BLOCKS - usable) as u32;
                }
            }
        }
    }
}

enum SectionScan {
    Absent,
    Uniform(u16),
    Palette { distinct_len: usize },
}

fn scan_section(
    snapshot: Option<&Data<SECTION_SIZE_BLOCKS>>,
    rank: &[u16],
    scratch: &mut Scratch,
) -> SectionScan {
    let Some(data) = snapshot else {
        return SectionScan::Absent;
    };
    match data {
        Data::Single(atom) => SectionScan::Uniform(rank[*atom as usize]),
        Data::Paletted(paletted) => {
            let bytes = &paletted.packed_long_array[..];
            let seen_mark = scratch.next_generation();
            let seen = &mut *scratch.seen;
            let distinct = &mut *scratch.distinct;
            let mut distinct_len = 0usize;
            let usable;
            if paletted.palette.direct() {
                usable = (bytes.len() / 2).min(SECTION_SIZE_BLOCKS);
                for pair in bytes.chunks_exact(2).take(usable) {
                    let atom = u16::from_le_bytes([pair[0], pair[1]]) as usize;
                    if seen[atom] != seen_mark {
                        seen[atom] = seen_mark;
                        distinct[distinct_len] = rank[atom];
                        distinct_len += 1;
                    }
                }
            } else {
                let bits = paletted.palette.bits_per_entry;
                let (packed, cells) = usable_span(bits, bytes);
                usable = cells;
                extract_indices(bits, packed, &mut scratch.indices[..]);
                let palette = &paletted.palette.palette;
                for &index in scratch.indices[..usable].iter() {
                    let atom = palette[index as usize] as usize;
                    if seen[atom] != seen_mark {
                        seen[atom] = seen_mark;
                        distinct[distinct_len] = rank[atom];
                        distinct_len += 1;
                    }
                }
            }
            if usable < SECTION_SIZE_BLOCKS && seen[0] != seen_mark {
                seen[0] = seen_mark;
                distinct[distinct_len] = rank[0];
                distinct_len += 1;
            }
            if distinct_len == 1 {
                return SectionScan::Uniform(distinct[0]);
            }
            distinct[..distinct_len].sort_unstable_by_key(|&a| rank[a as usize]);
            SectionScan::Palette { distinct_len }
        }
    }
}

fn encode_grid(
    mut voxels: Vec<u16>,
    section_count: usize,
    data: &RegionData,
    ranked: &[u16],
    rank: &[u16],
    scratch: &mut Scratch,
) -> Result<Vec<u8>, ModelError> {
    let mut side_streams = SideStreams {
        headers: Vec::new(),
        uniforms: Vec::new(),
        palettes: Vec::new(),
    };
    let mut encoder = Encoder::default();
    let mut modeler = Modeler::new(voxels.len());

    for section_y in 0..section_count {
        for segment_z in 0..SEGMENT_SIDE {
            for segment_x in 0..SEGMENT_SIDE {
                let slot = segment_x * SEGMENT_SIDE + segment_z;
                let pos = SectionPos { slot, section_y };
                let snapshot = data.segments[slot].as_ref().and_then(|segment| {
                    segment.block_sections[section_y]
                        .snapshots()
                        .first()
                        .map(|snapshot| &snapshot.data.data)
                });
                match scan_section(snapshot, rank, scratch) {
                    SectionScan::Absent => {
                        side_streams.headers.push(0);
                        put_u16_le(&mut side_streams.uniforms, rank[0]);
                    }
                    SectionScan::Uniform(value) => {
                        side_streams.headers.push(0);
                        put_u16_le(&mut side_streams.uniforms, value);
                    }
                    SectionScan::Palette { distinct_len } => {
                        let palette_bits = bit_depth(distinct_len);
                        side_streams.headers.push(palette_bits as u8);
                        put_u16_le(&mut side_streams.palettes, distinct_len as u16);
                        let palette = &scratch.distinct[..distinct_len];
                        for &atom in palette {
                            put_u16_le(&mut side_streams.palettes, atom);
                        }
                        modeler.encode_section(
                            &mut encoder,
                            &mut voxels,
                            pos,
                            palette,
                            palette_bits,
                        )?;
                    }
                }
            }
        }
    }
    let arithmetic = encoder.finish();

    let mut out = Vec::new();
    put_u64_le(&mut out, voxels.len() as u64);
    put_u8(&mut out, MODE);
    put_u32_le(&mut out, ranked.len() as u32);
    for &atom in ranked {
        put_u16_le(&mut out, atom);
    }
    put_u64_le(&mut out, side_streams.headers.len() as u64);
    put_u64_le(&mut out, side_streams.uniforms.len() as u64);
    put_u64_le(&mut out, side_streams.palettes.len() as u64);
    put_bytes(&mut out, &side_streams.headers);
    put_bytes(&mut out, &side_streams.uniforms);
    put_bytes(&mut out, &side_streams.palettes);
    put_bytes(&mut out, &arithmetic);
    Ok(out)
}

struct ResidualCtx<'a> {
    neighbors: &'a [u16; CHAIN_SLOTS],
    head: &'a HeadLite,
    chain: &'a mut ChainScratch,
    palette: &'a [u16],
    palette_bits: usize,
    section_y: usize,
}

fn build_grid(
    data: &RegionData,
    section_count: usize,
    rank: &[u16],
    scratch: &mut Scratch,
) -> Vec<u16> {
    let mut voxels = vec![rank[0]; SIDE * SIDE * section_count * SECTION_SIDE];
    for slot in 0..SEGMENTS_PER_REGION {
        let Some(segment) = &data.segments[slot] else {
            continue;
        };
        for section_y in 0..section_count {
            let Some(snapshot) = segment.block_sections[section_y].snapshots().first() else {
                continue;
            };
            let origin = section_origin(slot, section_y);
            match &snapshot.data.data {
                Data::Single(atom) => {
                    fill_uniform(&mut voxels, origin, rank[*atom as usize]);
                }
                Data::Paletted(paletted) => {
                    if paletted.palette.direct() {
                        let values = snapshot.data.unpack();
                        fill_section_mapped(&mut voxels, origin, &values, rank);
                    } else {
                        let bits = paletted.palette.bits_per_entry;
                        let (packed, usable) = usable_span(bits, &paletted.packed_long_array[..]);
                        let palette = &paletted.palette.palette;
                        for (entry, &atom) in scratch.lut[..palette.len()].iter_mut().zip(palette.iter()) {
                            *entry = rank[atom as usize];
                        }
                        extract_indices(bits, packed, &mut scratch.indices[..]);
                        fill_section_lut(
                            &mut voxels,
                            origin,
                            &scratch.indices[..],
                            &scratch.lut,
                            usable,
                            rank[0],
                        );
                    }
                }
            }
        }
    }
    voxels
}

pub(crate) struct GridUniforms {
    pub(crate) rank: Vec<Option<u16>>,
}

pub(crate) fn decode_grid(
    payload: &PooledBytes,
    section_count: usize,
) -> Result<(Vec<u16>, Vec<u16>, GridUniforms), ModelError> {
    let expected_voxels = SIDE * SIDE * section_count * SECTION_SIDE;
    let mut cursor = ByteCursor::new(payload.clone());
    let total_voxels = cursor.read_u64()? as usize;
    if total_voxels != expected_voxels {
        return Err(ModelError::VoxelCountMismatch {
            actual: total_voxels,
            expected: expected_voxels,
        });
    }
    let mode = cursor.read_u8()?;
    if mode != MODE {
        return Err(ModelError::UnsupportedMode(mode));
    }
    let remap_len = cursor.read_u32()? as usize;
    let remap_bytes = cursor.take_slice(remap_len * 2)?;
    let mut ranked = vec![0u16; remap_len];
    for (entry, bytes) in ranked.iter_mut().zip(remap_bytes.chunks_exact(2)) {
        *entry = u16::from_le_bytes([bytes[0], bytes[1]]);
    }
    let headers_len = cursor.read_u64()? as usize;
    let uniforms_len = cursor.read_u64()? as usize;
    let palettes_len = cursor.read_u64()? as usize;
    let mut headers = ByteCursor::new(cursor.take_slice(headers_len)?);
    let mut uniforms = ByteCursor::new(cursor.take_slice(uniforms_len)?);
    let mut palettes = ByteCursor::new(cursor.take_slice(palettes_len)?);
    let arithmetic = &cursor.data[cursor.pos..];

    let mut decoder = Decoder::new(arithmetic);
    let mut modeler = Modeler::new(expected_voxels);
    let mut palette_buf = Box::new([0u16; SECTION_SIZE_BLOCKS]);
    let mut voxels = vec![0u16; expected_voxels];
    let mut grid_uniforms = GridUniforms {
        rank: vec![None; SEGMENTS_PER_REGION * section_count],
    };

    for section_y in 0..section_count {
        for segment_z in 0..SEGMENT_SIDE {
            for segment_x in 0..SEGMENT_SIDE {
                let slot = segment_x * SEGMENT_SIDE + segment_z;
                let palette_bits = headers.read_u8()? as usize;
                if palette_bits == 0 {
                    let value = uniforms.read_u16()? as usize;
                    if value >= remap_len {
                        return Err(ModelError::UniformRankOutOfRange {
                            rank: value,
                            len: remap_len,
                        });
                    }
                    fill_uniform(
                        &mut voxels,
                        SectionPos { slot, section_y }.origin(),
                        value as u16,
                    );
                    grid_uniforms.rank[slot * section_count + section_y] = Some(value as u16);
                    continue;
                }
                if palette_bits > MAX_BIT_DEPTH {
                    return Err(ModelError::InvalidBitDepth(palette_bits));
                }
                let palette_len = palettes.read_u16()? as usize;
                if palette_len < 2
                    || palette_len > palette_buf.len()
                    || bit_depth(palette_len) != palette_bits
                {
                    return Err(ModelError::PaletteInconsistent {
                        len: palette_len,
                        bit_depth: palette_bits,
                    });
                }
                for entry in palette_buf[..palette_len].iter_mut() {
                    *entry = palettes.read_u16()?;
                    if *entry as usize >= remap_len {
                        return Err(ModelError::PaletteRankOutOfRange {
                            rank: *entry as usize,
                            len: remap_len,
                        });
                    }
                }
                let palette = &palette_buf[..palette_len];
                modeler.decode_section(
                    &mut decoder,
                    &mut voxels,
                    SectionPos { slot, section_y },
                    palette,
                    palette_bits,
                )?;
            }
        }
    }
    Ok((voxels, ranked, grid_uniforms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::DimensionType;
    use crate::io::serialize::experimental::models::spatial::for_each_section_cell;
    use crate::raw::SegmentData;
    use crate::region::delta::PackedDeltaData;
    use crate::region::packed_data::{PackedData, PackedSnapshot};
    use crate::region::segment_info::{SegmentState, SegmentStateType};
    use crate::region::tile_entities::DeltaTileEntityData;
    use crate::version::Version;

    fn synthetic_grid(section_count: usize, seed: u64) -> Vec<u16> {
        let len = SIDE * SIDE * section_count * SECTION_SIDE;
        let mut voxels = vec![0u16; len];
        let mut state = seed | 1;
        for section_y in 0..section_count {
            for slot in 0..SEGMENTS_PER_REGION {
                for local_y in 0..SECTION_SIDE {
                    for local_z in 0..SECTION_SIDE {
                        for local_x in 0..SECTION_SIDE {
                            state = state
                                .wrapping_mul(6364136223846793005)
                                .wrapping_add(1442695040888963407);
                            let run = ((state >> 33) % 7 + 1) as usize;
                            let value = ((state >> 40) % 8) as u16;
                            let (origin_x, origin_z, origin_y) = section_origin(slot, section_y);
                            let y = origin_y + local_y;
                            let z = origin_z + local_z;
                            for x in (origin_x + local_x)
                                ..(origin_x + local_x + run).min(origin_x + SECTION_SIDE)
                            {
                                voxels[y * Y_STRIDE + z * Z_STRIDE + x] = value;
                            }
                        }
                    }
                }
            }
        }
        voxels
    }

    fn region_data_from_grid(grid: &[u16], section_count: usize) -> RegionData {
        let mut segments: [Option<SegmentData>; SEGMENTS_PER_REGION] =
            std::array::from_fn(|_| None);
        for (slot, segment_slot) in segments.iter_mut().enumerate() {
            let mut block_sections = Vec::with_capacity(section_count);
            for section_y in 0..section_count {
                let mut cells = [0u16; SECTION_SIZE_BLOCKS];
                for_each_section_cell(section_origin(slot, section_y), |idx, i| {
                    cells[i] = grid[idx];
                });
                block_sections.push(PackedDeltaData::new(vec![PackedSnapshot {
                    data: PackedData::pack(&cells),
                    timestamp: 0,
                }]));
            }
            *segment_slot = Some(SegmentData {
                block_sections,
                biome_sections: (0..section_count).map(|_| PackedDeltaData::default()).collect(),
                states: vec![SegmentState {
                    state_type: SegmentStateType::New,
                    timestamp: 0,
                }],
                tile_entities: DeltaTileEntityData::default(),
            });
        }
        RegionData {
            version: Version::default(),
            protocol_version: 769,
            dimension: DimensionType::Overworld,
            segments,
        }
    }

    #[test]
    fn grid_roundtrip_synthetic() {
        let original = synthetic_grid(3, 0xABCDEF);
        let data = region_data_from_grid(&original, 3);
        let payload = encode_region(&data, 3).unwrap();
        let (mut decoded, ranked_out, _) =
            decode_grid(&PooledBytes::from_vec(payload), 3).unwrap();
        for value in &mut decoded {
            *value = ranked_out[*value as usize];
        }

        let pos = original
            .iter()
            .zip(&decoded)
            .position(|(a, palette_bits)| a != palette_bits);
        assert_eq!(pos, None, "first mismatch at {pos:?}");
    }
}
