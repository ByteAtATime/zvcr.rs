use std::cell::RefCell;

use crate::io::serialize::error::{MAX_TILE_ENTITY_LIST_LENGTH, MAX_TILE_ENTITY_NBT_LENGTH};
use crate::io::serialize::primitives::{put_bytes, put_u8, put_u32_le, put_u64_le};
use crate::raw::RegionData;
use crate::region::tile_entities::{TileEntityDelta, TileEntityDeltaMap, TileEntityPosition};

use super::Streams;

thread_local! {
    static TILE_POSITIONS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn write(streams: &mut Streams, data: &RegionData) -> Result<(), String> {
    for segment in data.segments.iter().flatten() {
        let list_count = segment.tile_entities.reverse_deltas.len();
        if list_count as u64 > MAX_TILE_ENTITY_LIST_LENGTH {
            return Err(format!(
                "tile entity list count {list_count} exceeds {MAX_TILE_ENTITY_LIST_LENGTH}"
            ));
        }
        put_u32_le(&mut streams.tile_entities, list_count as u32);
        for list_delta in &segment.tile_entities.reverse_deltas {
            let entry_count = list_delta.deltas.len();
            if entry_count as u64 > MAX_TILE_ENTITY_LIST_LENGTH {
                return Err(format!(
                    "tile entity entry count {entry_count} exceeds {MAX_TILE_ENTITY_LIST_LENGTH}"
                ));
            }
            put_u64_le(&mut streams.tile_entities, list_delta.timestamp as u64);
            put_u32_le(&mut streams.tile_entities, entry_count as u32);
            write_tile_entries(&mut streams.tile_entities, &list_delta.deltas)?;
        }
    }
    Ok(())
}

fn write_tile_entries(out: &mut Vec<u8>, deltas: &TileEntityDeltaMap) -> Result<(), String> {
    TILE_POSITIONS.with(|cell| -> Result<(), String> {
        let positions = &mut *cell.borrow_mut();
        positions.clear();
        positions.extend(deltas.keys().map(|pos| pos.packed()));
        positions.sort_unstable();
        for &packed in positions.iter() {
            put_u32_le(out, packed);
        }
        for &packed in positions.iter() {
            let delta = &deltas[&TileEntityPosition::unpack(packed)];
            put_u8(out, u8::from(matches!(delta, TileEntityDelta::Put(_))));
        }
        for &packed in positions.iter() {
            if let TileEntityDelta::Put(tile) = &deltas[&TileEntityPosition::unpack(packed)] {
                put_u32_le(out, tile.tile_type);
            }
        }
        for &packed in positions.iter() {
            if let TileEntityDelta::Put(tile) = &deltas[&TileEntityPosition::unpack(packed)] {
                let nbt_length = tile.nbt.len();
                if nbt_length as u64 > MAX_TILE_ENTITY_NBT_LENGTH {
                    return Err(format!(
                        "tile entity nbt length {nbt_length} exceeds {MAX_TILE_ENTITY_NBT_LENGTH}"
                    ));
                }
                put_u32_le(out, nbt_length as u32);
            }
        }
        for &packed in positions.iter() {
            if let TileEntityDelta::Put(tile) = &deltas[&TileEntityPosition::unpack(packed)] {
                put_bytes(out, &tile.nbt);
            }
        }
        Ok(())
    })
}
