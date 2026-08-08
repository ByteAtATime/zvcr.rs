use crate::definitions::*;
use crate::dimension::DimensionType;
use crate::region::delta::PackedDeltaData;
use crate::region::delta_sequence::DeltaSequence;
use crate::region::packed_data::PackedSnapshot;
use crate::region::segment_info::SegmentInfo;
use crate::region::tile_entities::DeltaTileEntityData;
use crate::region::unpacked_view::UnpackedData;
use std::sync::Arc;

pub const MAX_SECTION_COUNT: usize = 24;

#[derive(Debug, Clone)]
pub struct DeltaSections<const UNPACKED_SIZE: usize> {
    pub sections: [PackedDeltaData<UNPACKED_SIZE>; MAX_SECTION_COUNT],
    pub section_count: usize,
}

impl<const UNPACKED_SIZE: usize> DeltaSections<UNPACKED_SIZE> {
    pub fn new(section_count: usize) -> Self {
        Self {
            sections: std::array::from_fn(|_| PackedDeltaData::default()),
            section_count,
        }
    }

    pub fn snapshot_from(&self, timestamp: i64) -> Option<Vec<UnpackedData<UNPACKED_SIZE>>> {
        let mut snapshots = Vec::with_capacity(self.section_count);
        for i in 0..self.section_count {
            snapshots.push(self.sections[i].snapshot_from(timestamp)?);
        }
        Some(snapshots)
    }

    pub fn latest_snapshot(&self) -> Option<(Vec<UnpackedData<UNPACKED_SIZE>>, i64)> {
        let mut snapshots = Vec::with_capacity(self.section_count);
        let mut earliest = i64::MAX;

        for i in 0..self.section_count {
            let latest = self.sections[i].latest_snapshot()?;
            earliest = earliest.min(latest.timestamp);
            snapshots.push(latest.data.unpack());
        }
        Some((snapshots, earliest))
    }

    pub fn update_sections(&mut self, section_updates: &[PackedSnapshot<UNPACKED_SIZE>]) -> usize {
        let mut changes = 0;
        for (i, update) in section_updates.iter().enumerate() {
            if i >= self.section_count {
                break;
            }
            changes += self.sections[i]
                .insert_snapshot(update.clone())
                .unwrap_or(0);
        }
        changes
    }
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub section_count: usize,
    pub block_sections: DeltaSections<SECTION_SIZE_BLOCKS>,
    pub biome_sections: DeltaSections<SECTION_SIZE_BIOMES>,
    pub info: SegmentInfo,
    pub tile_entities: DeltaTileEntityData,
}

impl Segment {
    pub fn new(dimension: DimensionType) -> Self {
        Self::with_section_count(dimension.section_count())
    }

    pub fn with_section_count(section_count: usize) -> Self {
        Self {
            section_count,
            block_sections: DeltaSections::new(section_count),
            biome_sections: DeltaSections::new(section_count),
            info: SegmentInfo::default(),
            tile_entities: DeltaTileEntityData::default(),
        }
    }
}

pub type SegmentMaybe = Option<Arc<Segment>>;

#[derive(Debug, Clone)]
pub struct Region {
    pub segments: [SegmentMaybe; SEGMENTS_PER_REGION],
    pub protocol_version: u16,
}

impl Region {
    pub fn new(protocol_version: u16) -> Self {
        Self {
            segments: std::array::from_fn(|_| None),
            protocol_version,
        }
    }

    pub fn get(&self, x: u8, z: u8) -> &SegmentMaybe {
        &self.segments[Self::segment_index(x, z)]
    }

    pub fn set(&mut self, x: u8, z: u8, segment: SegmentMaybe) {
        let idx = Self::segment_index(x, z);
        self.segments[idx] = segment;
    }

    pub fn segment_index(x: u8, z: u8) -> usize {
        assert!(
            (x as usize) < REGION_SIDELENGTH_SEGMENTS && (z as usize) < REGION_SIDELENGTH_SEGMENTS
        );
        (x as usize) * REGION_SIDELENGTH_SEGMENTS + (z as usize)
    }
}
