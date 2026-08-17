use crate::io::compression::compress_zstd_parts;
use crate::io::file_location::EXTENSION;
use crate::io::serialize::primitives::{put_bytes, put_u16_le, put_u32_le, put_u64_le, put_u8};
use crate::raw::{RegionData, SegmentData};
use crate::region::packed_data::{Data, PackedSnapshot};
use crate::region::tile_entities::{DeltaTileEntityData, TileEntityDelta, TileEntityPosition};

pub(crate) fn serialize_region_data(data: &RegionData, level: i32) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    for slot in &data.segments {
        put_u8(&mut body, u8::from(slot.is_some()));
    }
    for segment in data.segments.iter().flatten() {
        write_segment(&mut body, segment);
    }

    let mut out = Vec::with_capacity(EXTENSION.len() + 4);
    put_bytes(&mut out, EXTENSION.as_bytes());
    put_u8(&mut out, data.version as u8);
    put_u8(&mut out, data.dimension as u8);
    put_u16_le(&mut out, data.protocol_version);
    let compressed = compress_zstd_parts(&[&body], level)?;
    out.extend_from_slice(&compressed);
    Ok(out)
}

fn write_segment(body: &mut Vec<u8>, segment: &SegmentData) {
    put_u64_le(body, segment.block_sections.len() as u64);
    for section in &segment.block_sections {
        put_u64_le(body, section.snapshots().len() as u64);
        for snapshot in section.snapshots() {
            write_snapshot(body, snapshot);
        }
    }

    put_u64_le(body, segment.biome_sections.len() as u64);
    for section in &segment.biome_sections {
        put_u64_le(body, section.snapshots().len() as u64);
        for snapshot in section.snapshots() {
            write_snapshot(body, snapshot);
        }
    }

    put_u64_le(body, segment.states.len() as u64);
    for state in &segment.states {
        put_u8(body, state.state_type as u8);
        put_u64_le(body, state.timestamp as u64);
    }

    write_tile_entities(body, &segment.tile_entities);
}

fn write_snapshot<const UNPACKED_SIZE: usize>(body: &mut Vec<u8>, snapshot: &PackedSnapshot<UNPACKED_SIZE>) {
    put_u64_le(body, snapshot.timestamp as u64);
    match &snapshot.data.data {
        Data::Single(atom) => {
            put_u8(body, 0);
            put_u16_le(body, *atom);
        }
        Data::Paletted(paletted) => {
            put_u8(body, 1);
            put_u16_le(body, paletted.palette.length() as u16);
            for atom in paletted.palette.palette.iter() {
                put_u16_le(body, *atom);
            }
            put_u64_le(body, (paletted.packed_long_array.len() / 8) as u64);
            put_bytes(body, &paletted.packed_long_array);
        }
    }
}

fn write_tile_entities(body: &mut Vec<u8>, data: &DeltaTileEntityData) {
    put_u64_le(body, data.reverse_deltas.len() as u64);
    for list_delta in &data.reverse_deltas {
        put_u64_le(body, list_delta.timestamp as u64);
        put_u64_le(body, list_delta.deltas.len() as u64);
        let mut entries: Vec<(&TileEntityPosition, &TileEntityDelta)> = list_delta.deltas.iter().collect();
        entries.sort_unstable_by_key(|(pos, _)| pos.packed());
        for (pos, delta) in entries {
            put_u32_le(body, pos.packed());
            match delta {
                TileEntityDelta::Erase => put_u8(body, 0),
                TileEntityDelta::Put(tile) => {
                    put_u8(body, 1);
                    put_u32_le(body, tile.tile_type);
                    put_u64_le(body, tile.nbt.len() as u64);
                    put_bytes(body, &tile.nbt);
                }
            }
        }
    }
}
