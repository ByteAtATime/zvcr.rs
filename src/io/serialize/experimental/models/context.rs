use crate::definitions::{SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::io::buffer::PooledBytes;
use crate::io::serialize::experimental::models::error::ModelError;
use crate::io::serialize::experimental::models::predictor::{
    CHAIN_ORDER, CHAIN_SLOTS, CHAIN_TABLE_MASK, HeadLite, HeadState, MAX_BIT_DEPTH, NONE,
    PRIMARY_TABLE_MASK, Predictor, SectionMetadata, TREE_BAND_TABLE_MASK, adapt, adapt_weights,
    combine, gather_neighbors, gather_neighbors_fast, mix_logits,
};
use crate::io::serialize::experimental::models::range::{Decoder, Encoder};
use crate::io::serialize::experimental::models::spatial::{
    SECTION_SIDE, SEGMENT_SIDE, SIDE, SectionPos, Y_STRIDE, Z_STRIDE, fill_section, fill_uniform,
    for_each_section_cell, section_origin,
};
use crate::io::serialize::primitives::{
    ByteCursor, put_bytes, put_u8, put_u16_le, put_u32_le, put_u64_le,
};
use crate::raw::RegionData;
use crate::region::palette::ATOM_COUNT;

const MODE: u8 = 1;

fn bit_depth(distinct_len: usize) -> usize {
    (usize::BITS - (distinct_len - 1).leading_zeros()) as usize
}

struct Modeler {
    predictor: Predictor,
    inverse: Box<[u16]>,
}

impl Modeler {
    fn new() -> Self {
        Self {
            predictor: Predictor::new(),
            inverse: vec![0u16; ATOM_COUNT].into_boxed_slice(),
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
        let mut candidates = [NONE; CHAIN_SLOTS];
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
                    let head = self
                        .predictor
                        .encode_head(encoder, section, &mut state, truth);
                    if head.bit == 0 {
                        self.encode_residual(
                            encoder,
                            truth,
                            ResidualCtx {
                                neighbors: &state.neighbors,
                                head: &head,
                                candidates: &mut candidates,
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
        let mut candidates = [NONE; CHAIN_SLOTS];
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
                    let head = self.predictor.decode_head(decoder, section, &mut state);
                    let value = if head.bit != 0 {
                        head.primary_value
                    } else {
                        self.decode_residual(
                            decoder,
                            ResidualCtx {
                                neighbors: &state.neighbors,
                                head: &head,
                                candidates: &mut candidates,
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
            if ctx.candidates[..candidate_count].contains(&candidate) {
                continue;
            }
            ctx.candidates[candidate_count] = candidate;
            let chain_ctx = (combine(
                candidate as u32,
                candidate_count as u32 | (ctx.head.mask << 8),
            ) & CHAIN_TABLE_MASK) as usize;
            let prob = self.predictor.chain[slot][chain_ctx] as u32;
            let bit = (truth == candidate) as u32;
            encoder.encode(prob, bit);
            adapt(&mut self.predictor.chain[slot][chain_ctx], bit);
            if bit != 0 {
                return Ok(candidate);
            }
            candidate_count += 1;
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
            if ctx.candidates[..candidate_count].contains(&candidate) {
                continue;
            }
            ctx.candidates[candidate_count] = candidate;
            let chain_ctx = (combine(
                candidate as u32,
                candidate_count as u32 | (ctx.head.mask << 8),
            ) & CHAIN_TABLE_MASK) as usize;
            let prob = self.predictor.chain[slot][chain_ctx] as u32;
            let bit = decoder.decode(prob);
            adapt(&mut self.predictor.chain[slot][chain_ctx], bit);
            if bit != 0 {
                return Ok(candidate);
            }
            candidate_count += 1;
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
            adapt(&mut self.predictor.tree[0][spatial_tree_ctx], bit);
            adapt(&mut self.predictor.tree[1][bitpos_tree_ctx], bit);
            adapt(&mut self.predictor.tree_band[height_tree_ctx], bit);
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
            adapt(&mut self.predictor.tree[0][spatial_tree_ctx], bit);
            adapt(&mut self.predictor.tree[1][bitpos_tree_ctx], bit);
            adapt(&mut self.predictor.tree_band[height_tree_ctx], bit);
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
    hist: Box<[u32]>,
    distinct: Box<[u16]>,
    palette: Box<[u16]>,
}

impl Scratch {
    fn new() -> Self {
        Self {
            hist: vec![0u32; ATOM_COUNT].into_boxed_slice(),
            distinct: vec![0u16; SECTION_SIZE_BLOCKS].into_boxed_slice(),
            palette: vec![0u16; SECTION_SIZE_BLOCKS].into_boxed_slice(),
        }
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
    let voxels = build_grid(data, section_count);
    encode_grid(voxels, section_count)
}

fn encode_grid(mut voxels: Vec<u16>, section_count: usize) -> Result<Vec<u8>, ModelError> {
    let mut hist = vec![0u32; ATOM_COUNT];
    for &value in &voxels {
        hist[value as usize] += 1;
    }
    let mut ranked: Vec<u16> = (0..ATOM_COUNT)
        .filter(|&atom| hist[atom] != 0)
        .map(|atom| atom as u16)
        .collect();
    ranked.sort_unstable_by_key(|&atom| (std::cmp::Reverse(hist[atom as usize]), atom));
    let mut rank = vec![u16::MAX; ATOM_COUNT];
    for (r, &atom) in ranked.iter().enumerate() {
        rank[atom as usize] = r as u16;
    }
    for value in &mut voxels {
        *value = rank[*value as usize];
    }

    let mut side_streams = SideStreams {
        headers: Vec::new(),
        uniforms: Vec::new(),
        palettes: Vec::new(),
    };
    let mut encoder = Encoder::default();
    let mut modeler = Modeler::new();
    let mut scratch = Scratch::new();

    for section_y in 0..section_count {
        for segment_z in 0..SEGMENT_SIDE {
            for segment_x in 0..SEGMENT_SIDE {
                let slot = segment_x * SEGMENT_SIDE + segment_z;
                encode_section(
                    &mut modeler,
                    &mut encoder,
                    &mut voxels,
                    &rank,
                    &mut scratch,
                    SectionPos { slot, section_y },
                    &mut side_streams,
                )?;
            }
        }
    }
    let arithmetic = encoder.finish();

    let mut out = Vec::new();
    put_u64_le(&mut out, voxels.len() as u64);
    put_u8(&mut out, MODE);
    put_u32_le(&mut out, ranked.len() as u32);
    for &atom in &ranked {
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
    candidates: &'a mut [u16; CHAIN_SLOTS],
    palette: &'a [u16],
    palette_bits: usize,
    section_y: usize,
}

fn encode_section(
    modeler: &mut Modeler,
    encoder: &mut Encoder,
    voxels: &mut [u16],
    rank: &[u16],
    scratch: &mut Scratch,
    pos: SectionPos,
    side_streams: &mut SideStreams,
) -> Result<(), ModelError> {
    let origin = pos.origin();
    let hist = &mut *scratch.hist;
    let distinct = &mut *scratch.distinct;
    let mut distinct_len = 0usize;
    for_each_section_cell(origin, |idx, _| {
        let value = voxels[idx];
        let counter = &mut hist[value as usize];
        if *counter == 0 {
            distinct[distinct_len] = value;
            distinct_len += 1;
        }
        *counter += 1;
    });
    for i in 0..distinct_len {
        hist[distinct[i] as usize] = 0;
    }
    if distinct_len == 1 {
        side_streams.headers.push(0);
        put_u16_le(&mut side_streams.uniforms, distinct[0]);
        return Ok(());
    }
    distinct[..distinct_len].sort_unstable_by_key(|&a| rank[a as usize]);
    let palette_bits = bit_depth(distinct_len);
    side_streams.headers.push(palette_bits as u8);
    put_u16_le(&mut side_streams.palettes, distinct_len as u16);
    for &atom in &distinct[..distinct_len] {
        put_u16_le(&mut side_streams.palettes, atom);
    }
    scratch.palette[..distinct_len].copy_from_slice(&distinct[..distinct_len]);
    modeler.encode_section(
        encoder,
        voxels,
        pos,
        &scratch.palette[..distinct_len],
        palette_bits,
    )
}

fn build_grid(data: &RegionData, section_count: usize) -> Vec<u16> {
    let mut voxels = vec![0u16; SIDE * SIDE * section_count * SECTION_SIDE];
    for slot in 0..SEGMENTS_PER_REGION {
        let Some(segment) = &data.segments[slot] else {
            continue;
        };
        for section_y in 0..section_count {
            let snapshots = segment.block_sections[section_y].snapshots();
            if snapshots.is_empty() {
                continue;
            }
            fill_section(
                &mut voxels,
                section_origin(slot, section_y),
                &snapshots[0].data.unpack(),
            );
        }
    }
    voxels
}

pub(crate) fn decode_grid(
    payload: &PooledBytes,
    section_count: usize,
) -> Result<(Vec<u16>, Vec<u16>), ModelError> {
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
    let mut modeler = Modeler::new();
    let mut palette_scratch = Scratch::new();
    let mut voxels = vec![0u16; expected_voxels];

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
                    continue;
                }
                if palette_bits > MAX_BIT_DEPTH {
                    return Err(ModelError::InvalidBitDepth(palette_bits));
                }
                let palette_len = palettes.read_u16()? as usize;
                if palette_len < 2
                    || palette_len > palette_scratch.palette.len()
                    || bit_depth(palette_len) != palette_bits
                {
                    return Err(ModelError::PaletteInconsistent {
                        len: palette_len,
                        bit_depth: palette_bits,
                    });
                }
                for entry in palette_scratch.palette[..palette_len].iter_mut() {
                    *entry = palettes.read_u16()?;
                    if *entry as usize >= remap_len {
                        return Err(ModelError::PaletteRankOutOfRange {
                            rank: *entry as usize,
                            len: remap_len,
                        });
                    }
                }
                let palette = &palette_scratch.palette[..palette_len];
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
    Ok((voxels, ranked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::serialize::experimental::models::spatial::{Y_STRIDE, Z_STRIDE};

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

    #[test]
    fn grid_roundtrip_synthetic() {
        let original = synthetic_grid(3, 0xABCDEF);
        let payload = encode_grid(original.clone(), 3).unwrap();
        let (mut decoded, ranked) = decode_grid(&PooledBytes::from_vec(payload), 3).unwrap();
        for value in &mut decoded {
            *value = ranked[*value as usize];
        }

        let pos = original
            .iter()
            .zip(&decoded)
            .position(|(a, palette_bits)| a != palette_bits);
        assert_eq!(pos, None, "first mismatch at {pos:?}");
    }
}
