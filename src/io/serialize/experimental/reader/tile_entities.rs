use std::collections::HashMap;

use crate::io::serialize::error::{
    MAX_TILE_ENTITY_LIST_LENGTH, MAX_TILE_ENTITY_NBT_LENGTH, ReadError,
};
use crate::io::serialize::primitives::ByteCursor;
use crate::region::tile_entities::{
    DeltaTileEntityData, TileEntity, TileEntityDelta, TileEntityListDelta, TileEntityPosition,
};

pub(super) fn read(cursor: &mut ByteCursor) -> Result<DeltaTileEntityData, ReadError> {
    let list_count = cursor.read_u32()? as usize;
    if list_count as u64 > MAX_TILE_ENTITY_LIST_LENGTH {
        return Err(ReadError::LengthExceeded(format!(
            "tile entity list count {list_count} exceeds {MAX_TILE_ENTITY_LIST_LENGTH}"
        )));
    }
    let mut reverse_deltas = Vec::with_capacity(list_count);
    for _ in 0..list_count {
        let timestamp = cursor.read_u64()? as i64;
        let entry_count = cursor.read_u32()? as usize;
        if entry_count as u64 > MAX_TILE_ENTITY_LIST_LENGTH {
            return Err(ReadError::LengthExceeded(format!(
                "tile entity entry count {entry_count} exceeds {MAX_TILE_ENTITY_LIST_LENGTH}"
            )));
        }
        let mut positions = vec![0u32; entry_count];
        for position in positions.iter_mut() {
            *position = cursor.read_u32()?;
        }
        for window in positions.windows(2) {
            if window[0] >= window[1] {
                return Err(ReadError::Generic(format!(
                    "tile entity positions not strictly ascending: {} then {}",
                    window[0], window[1]
                )));
            }
        }
        let mut ops = vec![0u8; entry_count];
        cursor.read_exact(&mut ops)?;
        for &op in &ops {
            if op > 1 {
                return Err(ReadError::Generic(format!("invalid tile entity op {op}")));
            }
        }
        let put_count = ops.iter().filter(|&&op| op == 1).count();
        let mut tile_types = vec![0u32; put_count];
        for tile_type in tile_types.iter_mut() {
            *tile_type = cursor.read_u32()?;
        }
        let mut nbt_lengths = vec![0u32; put_count];
        for length in nbt_lengths.iter_mut() {
            let value = cursor.read_u32()?;
            if value as u64 > MAX_TILE_ENTITY_NBT_LENGTH {
                return Err(ReadError::LengthExceeded(format!(
                    "tile entity nbt length {value} exceeds {MAX_TILE_ENTITY_NBT_LENGTH}"
                )));
            }
            *length = value;
        }
        let mut deltas = HashMap::with_capacity(entry_count);
        let mut put_index = 0usize;
        for (&position, &op) in positions.iter().zip(ops.iter()) {
            let pos = TileEntityPosition::unpack(position);
            let delta = if op == 0 {
                TileEntityDelta::Erase
            } else {
                let nbt = cursor.take_slice(nbt_lengths[put_index] as usize)?.to_vec();
                let tile = TileEntity {
                    tile_type: tile_types[put_index],
                    pos,
                    nbt,
                };
                put_index += 1;
                TileEntityDelta::Put(tile)
            };
            deltas.insert(pos, delta);
        }
        reverse_deltas.push(TileEntityListDelta { timestamp, deltas });
    }
    Ok(DeltaTileEntityData { reverse_deltas })
}
