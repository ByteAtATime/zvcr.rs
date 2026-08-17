use crate::definitions::{SECTION_SIZE_BIOMES, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use crate::dimension::DimensionType;
use crate::io::buffer::PooledBytes;
use crate::io::compression::decompress_zstd;
use crate::io::file_location::EXTENSION;
use crate::io::serialize::error::{
    ReadError, MAX_DELTA_LENGTH, MAX_PACKED_LENGTH, MAX_SEGMENT_STATES_LENGTH,
    MAX_TILE_ENTITY_LIST_LENGTH, MAX_TILE_ENTITY_NBT_LENGTH,
};
use crate::io::serialize::primitives::ByteCursor;
use crate::raw::{RegionData, SegmentData};
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PalettedData, PackedData, PackedSnapshot};
use crate::region::palette::{bits_per_entry, Palette, MAX_INDIRECT_PALETTE_SIZE, DIRECT_PALETTE};
use crate::region::segment::MAX_SECTION_COUNT;
use crate::region::segment_info::{SegmentState, SegmentStateType};
use crate::region::tile_entities::{
    DeltaTileEntityData, TileEntity, TileEntityDelta, TileEntityListDelta, TileEntityPosition,
};
use crate::version::Version;
use std::collections::HashMap;
use std::sync::Arc;

const HEADER_LENGTH: usize = EXTENSION.len() + 4;

pub(crate) fn deserialize_region_data(bytes: &[u8]) -> Result<RegionData, ReadError> {
    let mut header = ByteCursor::new(PooledBytes::from_vec(bytes.to_vec()));
    let magic = header.take_slice(EXTENSION.len())?;
    if &magic[..] != EXTENSION.as_bytes() {
        return Err(ReadError::HeaderMismatch);
    }
    let version_u8 = header.read_u8()?;
    let version = Version::from_u8(version_u8).ok_or(ReadError::InvalidVersion(version_u8))?;
    let dimension_u8 = header.read_u8()?;
    let dimension = DimensionType::from_u8(dimension_u8)
        .ok_or(ReadError::InvalidDimensionType(dimension_u8))?;
    let protocol_version = header.read_u16()?;

    let compressed = &bytes[HEADER_LENGTH..];
    let body = decompress_zstd(compressed).map_err(ReadError::Zstd)?;
    let mut cursor = ByteCursor::new(PooledBytes::from_vec(body));

    let mut presence = [false; SEGMENTS_PER_REGION];
    for flag in &mut presence {
        *flag = cursor.read_u8()? == 1;
    }

    let mut segments = std::array::from_fn(|_| None);
    for (slot, present) in segments.iter_mut().zip(presence) {
        if present {
            *slot = Some(read_segment(&mut cursor)?);
        }
    }

    Ok(RegionData {
        version,
        protocol_version,
        dimension,
        segments,
    })
}

fn read_segment(cursor: &mut ByteCursor) -> Result<SegmentData, ReadError> {
    let block_count = read_bounded_len(cursor, MAX_SECTION_COUNT as u64, "block section count")?;
    let mut block_sections = Vec::with_capacity(block_count);
    for _ in 0..block_count {
        block_sections.push(read_delta_data::<SECTION_SIZE_BLOCKS>(cursor)?);
    }

    let biome_count = read_bounded_len(cursor, MAX_SECTION_COUNT as u64, "biome section count")?;
    let mut biome_sections = Vec::with_capacity(biome_count);
    for _ in 0..biome_count {
        biome_sections.push(read_delta_data::<SECTION_SIZE_BIOMES>(cursor)?);
    }

    let state_count = read_bounded_len(cursor, MAX_SEGMENT_STATES_LENGTH, "segment state count")?;
    let mut states = Vec::with_capacity(state_count);
    for _ in 0..state_count {
        let state_type_u8 = cursor.read_u8()?;
        let state_type = SegmentStateType::from_u8(state_type_u8)
            .ok_or_else(|| ReadError::Generic(format!("invalid segment state type {state_type_u8}")))?;
        let timestamp = cursor.read_u64()? as i64;
        states.push(SegmentState { state_type, timestamp });
    }

    let tile_entities = read_tile_entities(cursor)?;

    Ok(SegmentData {
        block_sections,
        biome_sections,
        states,
        tile_entities,
    })
}

fn read_delta_data<const UNPACKED_SIZE: usize>(
    cursor: &mut ByteCursor,
) -> Result<PackedDeltaData<UNPACKED_SIZE>, ReadError> {
    let snapshot_count = read_bounded_len(cursor, MAX_DELTA_LENGTH, "snapshot count")?;
    let mut snapshots = Vec::with_capacity(snapshot_count);
    for _ in 0..snapshot_count {
        snapshots.push(read_snapshot(cursor)?);
    }
    Ok(PackedDeltaData::new(snapshots))
}

fn read_snapshot<const UNPACKED_SIZE: usize>(
    cursor: &mut ByteCursor,
) -> Result<PackedSnapshot<UNPACKED_SIZE>, ReadError> {
    let timestamp = cursor.read_u64()? as i64;
    let tag = cursor.read_u8()?;
    let data = match tag {
        0 => {
            let atom = cursor.read_u16()?;
            Data::Single(atom)
        }
        1 => {
            let palette_len = cursor.read_u16()? as usize;
            if palette_len > MAX_INDIRECT_PALETTE_SIZE {
                return Err(ReadError::LengthExceeded(format!("palette length {palette_len}")));
            }
            let palette = if palette_len == 0 {
                DIRECT_PALETTE.clone()
            } else {
                let mut atoms = Vec::with_capacity(palette_len);
                for _ in 0..palette_len {
                    atoms.push(cursor.read_u16()?);
                }
                Palette {
                    palette: Arc::from(atoms),
                    bits_per_entry: bits_per_entry(palette_len),
                }
            };
            let long_count = read_bounded_len(cursor, MAX_PACKED_LENGTH, "packed long count")?;
            let packed_long_array = cursor.take_slice(long_count * 8)?;
            Data::Paletted(PalettedData { packed_long_array, palette })
        }
        _ => return Err(ReadError::Generic(format!("invalid snapshot tag {tag}"))),
    };
    Ok(PackedSnapshot {
        data: PackedData { data },
        timestamp,
    })
}

fn read_tile_entities(cursor: &mut ByteCursor) -> Result<DeltaTileEntityData, ReadError> {
    let list_count = read_bounded_len(cursor, MAX_TILE_ENTITY_LIST_LENGTH, "tile entity list count")?;
    let mut reverse_deltas = Vec::with_capacity(list_count);
    for _ in 0..list_count {
        let timestamp = cursor.read_u64()? as i64;
        let entry_count = read_bounded_len(cursor, MAX_TILE_ENTITY_LIST_LENGTH, "tile entity count")?;
        let mut deltas = HashMap::with_capacity(entry_count);
        for _ in 0..entry_count {
            let packed_position = cursor.read_u32()?;
            let position = TileEntityPosition::unpack(packed_position);
            let op = cursor.read_u8()?;
            let delta = match op {
                0 => TileEntityDelta::Erase,
                1 => {
                    let tile_type = cursor.read_u32()?;
                    let nbt_len = read_bounded_len(cursor, MAX_TILE_ENTITY_NBT_LENGTH, "tile entity nbt length")?;
                    let nbt = cursor.take_slice(nbt_len)?.to_vec();
                    TileEntityDelta::Put(TileEntity { tile_type, pos: position, nbt })
                }
                _ => return Err(ReadError::Generic(format!("invalid tile entity op {op}"))),
            };
            deltas.insert(position, delta);
        }
        reverse_deltas.push(TileEntityListDelta { timestamp, deltas });
    }
    Ok(DeltaTileEntityData { reverse_deltas })
}

fn read_bounded_len(cursor: &mut ByteCursor, cap: u64, what: &str) -> Result<usize, ReadError> {
    let len = cursor.read_u64()?;
    if len > cap {
        return Err(ReadError::LengthExceeded(format!("{what} {len} exceeds {cap}")));
    }
    Ok(len as usize)
}
