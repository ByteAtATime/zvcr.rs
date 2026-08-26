mod descriptors;
mod snapshots;
mod stats;
mod tile_entities;

use crate::io::compression::compress_zstd_parts;
use crate::io::serialize::experimental::layout::{
    self, BUCKETS, Domain, FormatVersion, MAGIC, PART_COUNT, PRESENCE_BYTES,
};
use crate::io::serialize::experimental::models::context;
use crate::io::serialize::primitives::{put_bytes, put_u8, put_u16_le, put_u32_le, put_u64_le};
use crate::raw::RegionData;

pub(crate) fn serialize_region_data(data: &RegionData, level: i32) -> Result<Vec<u8>, String> {
    let section_count = validate_section_counts(data)?;
    let mut streams = Streams::new();

    let mut presence = [0u8; PRESENCE_BYTES];
    for (slot, segment) in data.segments.iter().enumerate() {
        if segment.is_some() {
            layout::set_presence_bit(&mut presence, slot);
        }
    }
    put_bytes(&mut streams.metadata, &presence);

    descriptors::write(&mut streams, data, section_count)?;
    let model = context::encode_region(data, section_count).map_err(|e| e.to_string())?;
    put_u32_le(&mut streams.model, model.len() as u32);
    put_bytes(&mut streams.model, &model);
    snapshots::write_domain(&mut streams, data, Domain::Block, |segment| {
        &segment.block_sections
    })?;
    snapshots::write_domain(&mut streams, data, Domain::Biome, |segment| {
        &segment.biome_sections
    })?;
    write_states(&mut streams.chunk_info, data)?;
    tile_entities::write(&mut streams, data)?;

    let mut out = Vec::with_capacity(layout::HEADER_LENGTH);
    put_bytes(&mut out, MAGIC.as_bytes());
    put_u8(&mut out, FormatVersion::V0_0_1.as_u8());
    put_u8(&mut out, data.dimension as u8);
    put_u16_le(&mut out, data.protocol_version);
    let compressed = compress_zstd_parts(&streams.parts(), level)?;
    out.extend_from_slice(&compressed);

    stats::emit_if_enabled(&streams, level);

    Ok(out)
}

struct Streams {
    metadata: Vec<u8>,
    model: Vec<u8>,
    global_palette: Vec<u8>,
    chunk_info: Vec<u8>,
    timestamps: Vec<u8>,
    singles: Vec<u8>,
    local_palettes: Vec<u8>,
    buckets: [Vec<u8>; BUCKETS],
    bucket_counts: [u64; BUCKETS],
    tile_entities: Vec<u8>,
}

impl Streams {
    fn new() -> Self {
        Self {
            metadata: Vec::new(),
            model: Vec::new(),
            global_palette: Vec::new(),
            chunk_info: Vec::new(),
            timestamps: Vec::new(),
            singles: Vec::new(),
            local_palettes: Vec::new(),
            buckets: std::array::from_fn(|_| Vec::new()),
            bucket_counts: [0; BUCKETS],
            tile_entities: Vec::new(),
        }
    }

    fn parts(&self) -> [&[u8]; PART_COUNT] {
        [
            &self.metadata,
            &self.model,
            &self.global_palette,
            &self.chunk_info,
            &self.timestamps,
            &self.singles,
            &self.local_palettes,
            &self.buckets[0],
            &self.buckets[1],
            &self.buckets[2],
            &self.buckets[3],
            &self.buckets[4],
            &self.buckets[5],
            &self.buckets[6],
            &self.buckets[7],
            &self.buckets[8],
            &self.tile_entities,
        ]
    }
}

fn validate_section_counts(data: &RegionData) -> Result<usize, String> {
    let section_count = data.dimension.section_count();
    for segment in data.segments.iter().flatten() {
        if segment.block_sections.len() != section_count {
            return Err(format!(
                "block section count {} does not match dimension section count {section_count}",
                segment.block_sections.len()
            ));
        }
        if segment.biome_sections.len() != section_count {
            return Err(format!(
                "biome section count {} does not match dimension section count {section_count}",
                segment.biome_sections.len()
            ));
        }
    }
    Ok(section_count)
}

fn write_states(out: &mut Vec<u8>, data: &RegionData) -> Result<(), String> {
    for segment in data.segments.iter().flatten() {
        let count = segment.states.len();
        if count > u16::MAX as usize {
            return Err(format!("segment state count {count} exceeds 65535"));
        }
        put_u16_le(out, count as u16);
        for state in &segment.states {
            put_u8(out, state.state_type as u8);
        }
        for state in &segment.states {
            put_u64_le(out, state.timestamp as u64);
        }
    }
    Ok(())
}
