use crate::definitions::SEGMENTS_PER_REGION;
use crate::io::serialize::error::{MAX_SEGMENT_STATES_LENGTH, ReadError};
use crate::io::serialize::experimental::layout::{self, PRESENCE_BYTES};
use crate::io::serialize::primitives::ByteCursor;
use crate::region::palette::ATOM_COUNT;
use crate::region::segment_info::{SegmentState, SegmentStateType};

pub(super) fn read_presence(
    cursor: &mut ByteCursor,
) -> Result<[bool; SEGMENTS_PER_REGION], ReadError> {
    let presence_bytes = cursor.read_bytes::<PRESENCE_BYTES>()?;
    let mut presence = [false; SEGMENTS_PER_REGION];
    for (slot, present) in presence.iter_mut().enumerate() {
        *present = layout::presence_bit(&presence_bytes, slot);
    }
    Ok(presence)
}

pub(super) fn read_descriptors(
    cursor: &mut ByteCursor,
    presence: &[bool; SEGMENTS_PER_REGION],
    section_count: usize,
) -> Result<Vec<u8>, ReadError> {
    let total_sections = SEGMENTS_PER_REGION * section_count;
    let mut packed = vec![0u8; (total_sections * 2).div_ceil(8)];
    cursor.read_exact(&mut packed)?;
    let mut descriptors = vec![0u8; total_sections];
    for scan in 0..total_sections {
        let descriptor = layout::descriptor_bits(&packed, scan);
        if descriptor > 2 {
            return Err(ReadError::Generic(format!(
                "invalid snapshot descriptor {descriptor}"
            )));
        }
        if descriptor != 0 && !presence[scan / section_count] {
            return Err(ReadError::Generic(format!(
                "descriptor {descriptor} for absent segment slot {}",
                scan / section_count
            )));
        }
        descriptors[scan] = descriptor;
    }
    Ok(descriptors)
}

pub(super) fn read_counts(
    cursor: &mut ByteCursor,
    descriptors: &[u8],
) -> Result<Vec<u16>, ReadError> {
    let mut counts = vec![0u16; descriptors.len()];
    for (count, &descriptor) in counts.iter_mut().zip(descriptors) {
        if descriptor == 0 {
            continue;
        }
        let value = cursor.read_u16()?;
        if value == 0 {
            return Err(ReadError::Generic(format!(
                "descriptor {descriptor} with zero snapshot count"
            )));
        }
        *count = value;
    }
    Ok(counts)
}

pub(super) fn read_delta_tags(
    cursor: &mut ByteCursor,
    counts: &[u16],
    descriptors: &[u8],
) -> Result<Vec<u8>, ReadError> {
    let total: usize = counts.iter().map(|&c| c as usize).sum();
    let newest = descriptors.iter().filter(|&&d| d != 0).count();
    let delta_count = total - newest;
    let mut tags = vec![0u8; delta_count];
    for tag in &mut tags {
        let value = cursor.read_u8()?;
        if value != 1 && value != 2 {
            return Err(ReadError::Generic(format!(
                "invalid delta snapshot tag {value}"
            )));
        }
        *tag = value;
    }
    Ok(tags)
}

pub(super) fn read_palette(cursor: &mut ByteCursor) -> Result<Vec<u16>, ReadError> {
    let length = cursor.read_u32()? as usize;
    if length > ATOM_COUNT {
        return Err(ReadError::LengthExceeded(format!(
            "global palette length {length} exceeds {ATOM_COUNT}"
        )));
    }
    let mut atoms = vec![0u16; length];
    for atom in atoms.iter_mut() {
        *atom = cursor.read_u16()?;
    }
    Ok(atoms)
}

pub(super) fn read_timestamps(
    cursor: &mut ByteCursor,
    count: usize,
) -> Result<Vec<i64>, ReadError> {
    let bytes = cursor.take_slice(count * 8)?;
    let timestamps: Vec<i64> = bytes
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap()) as i64)
        .collect();
    Ok(timestamps)
}

pub(super) fn read_singles(cursor: &mut ByteCursor, count: usize) -> Result<Vec<u16>, ReadError> {
    let bytes = cursor.take_slice(count * 2)?;
    let singles: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes(chunk.try_into().unwrap()))
        .collect();
    Ok(singles)
}

pub(super) fn read_states_by_segment(
    cursor: &mut ByteCursor,
    presence: &[bool; SEGMENTS_PER_REGION],
) -> Result<Vec<Vec<SegmentState>>, ReadError> {
    let mut states_by_segment = Vec::new();
    for &present in presence.iter() {
        if !present {
            continue;
        }
        let state_count = cursor.read_u16()? as usize;
        if state_count as u64 > MAX_SEGMENT_STATES_LENGTH {
            return Err(ReadError::LengthExceeded(format!(
                "segment state count {state_count} exceeds {MAX_SEGMENT_STATES_LENGTH}"
            )));
        }
        let mut states = Vec::with_capacity(state_count);
        for _ in 0..state_count {
            let state_type_u8 = cursor.read_u8()?;
            let state_type = SegmentStateType::from_u8(state_type_u8).ok_or_else(|| {
                ReadError::Generic(format!("invalid segment state type {state_type_u8}"))
            })?;
            states.push(SegmentState {
                state_type,
                timestamp: 0,
            });
        }
        for state in states.iter_mut() {
            state.timestamp = cursor.read_u64()? as i64;
        }
        states_by_segment.push(states);
    }
    Ok(states_by_segment)
}
