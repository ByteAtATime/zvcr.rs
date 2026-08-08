use crate::definitions::*;
use crate::time_utils::find_nearest_timestamp;

pub type LongArray = Vec<u64>;
pub type UnpackedData<const UNPACKED_SIZE: usize> = [SegmentAtom; UNPACKED_SIZE];

#[derive(Debug, Clone)]
pub struct UnpackedView<const UNPACKED_SIZE: usize> {
    pub unpacked: UnpackedData<UNPACKED_SIZE>,
    sidelength: u8,
}

impl<const UNPACKED_SIZE: usize> UnpackedView<UNPACKED_SIZE> {
    pub fn new(sidelength: u8, fill: SegmentAtom) -> Self {
        Self {
            unpacked: [fill; UNPACKED_SIZE],
            sidelength,
        }
    }

    pub fn from_data(sidelength: u8, unpacked: UnpackedData<UNPACKED_SIZE>) -> Self {
        Self {
            unpacked,
            sidelength,
        }
    }

    pub fn voxel(&self, x: u8, y: u8, z: u8) -> SegmentAtom {
        self.unpacked[self.unpacked_index(x, y, z)]
    }

    pub fn set_voxel(&mut self, x: u8, y: u8, z: u8, voxel: SegmentAtom) {
        let idx = self.unpacked_index(x, y, z);
        self.unpacked[idx] = voxel;
    }

    pub fn pack(&self) -> PackedData<UNPACKED_SIZE> {
        PackedData::pack(&self.unpacked)
    }

    pub fn pack_snapshot(&self, timestamp: i64) -> PackedSnapshot<UNPACKED_SIZE> {
        PackedSnapshot {
            data: self.pack(),
            timestamp,
        }
    }

    pub fn unpacked_index(&self, x: u8, y: u8, z: u8) -> usize {
        assert!(x < self.sidelength, "X coordinate out of bounds");
        assert!(y < self.sidelength, "Y coordinate out of bounds");
        assert!(z < self.sidelength, "Z coordinate out of bounds");
        (y as usize) * (self.sidelength as usize) * (self.sidelength as usize)
            + (z as usize) * (self.sidelength as usize)
            + (x as usize)
    }

    pub fn sidelength(&self) -> u8 {
        self.sidelength
    }
}

pub fn create_block_view(fill: SegmentAtom) -> UnpackedView<SECTION_SIZE_BLOCKS> {
    UnpackedView::new(SEGMENT_SIDELENGTH_BLOCKS as u8, fill)
}

pub fn create_biome_view(fill: SegmentAtom) -> UnpackedView<SECTION_SIZE_BIOMES> {
    UnpackedView::new(SEGMENT_SIDELENGTH_BIOMES as u8, fill)
}

pub const MAX_INDIRECT_PALETTE_SIZE: usize = u8::MAX as usize + 1;

pub fn bits_per_entry(palette_length: usize) -> usize {
    if palette_length <= 16 {
        4
    } else if palette_length <= MAX_INDIRECT_PALETTE_SIZE {
        8
    } else {
        16
    }
}

pub type VectorPalette = Vec<SegmentAtom>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Palette {
    pub palette: VectorPalette,
    pub bits_per_entry: usize,
}

impl Palette {
    pub fn length(&self) -> usize {
        self.palette.len()
    }

    pub fn direct(&self) -> bool {
        self.palette.is_empty()
    }
}

pub const DIRECT_PALETTE: Palette = Palette {
    palette: Vec::new(),
    bits_per_entry: 16,
};

pub fn build_palette<const UNPACKED_SIZE: usize>(
    data: &UnpackedData<UNPACKED_SIZE>,
    indices: &mut [u8; u16::MAX as usize + 1],
) -> Palette {
    let mut palette = Vec::new();
    let mut unique = vec![false; u16::MAX as usize + 1];

    for &atom in data.iter() {
        if unique[atom as usize] {
            continue;
        }
        unique[atom as usize] = true;
        if palette.len() >= MAX_INDIRECT_PALETTE_SIZE {
            return DIRECT_PALETTE;
        }
        indices[atom as usize] = palette.len() as u8;
        palette.push(atom);
    }

    let bpe = bits_per_entry(palette.len());
    Palette {
        palette,
        bits_per_entry: bpe,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PalettedData<const UNPACKED_SIZE: usize> {
    pub packed_long_array: LongArray,
    pub palette: Palette,
}

impl<const UNPACKED_SIZE: usize> PalettedData<UNPACKED_SIZE> {
    pub fn new(palette: Palette) -> Self {
        let values_per_long = 64 / palette.bits_per_entry;
        let packed_length = UNPACKED_SIZE.div_ceil(values_per_long);
        Self {
            packed_long_array: vec![0; packed_length],
            palette,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data<const UNPACKED_SIZE: usize> {
    Paletted(PalettedData<UNPACKED_SIZE>),
    Single(SegmentAtom),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedData<const UNPACKED_SIZE: usize> {
    pub data: Data<UNPACKED_SIZE>,
}

impl<const UNPACKED_SIZE: usize> PackedData<UNPACKED_SIZE> {
    pub fn pack(section_data: &UnpackedData<UNPACKED_SIZE>) -> Self {
        let mut indices = [0u8; u16::MAX as usize + 1];
        let palette = build_palette(section_data, &mut indices);

        if palette.length() == 1 {
            return Self {
                data: Data::Single(palette.palette[0]),
            };
        }

        let mut paletted_data = PalettedData::new(palette.clone());
        let bits = palette.bits_per_entry as u8;
        let mask = (1u64 << bits) - 1;
        let mut unpacked_index = 0;

        for cell_index in 0..paletted_data.packed_long_array.len() {
            let mut cell = 0u64;
            let mut bit_index = 0u8;
            while bit_index < 64 {
                if unpacked_index >= UNPACKED_SIZE {
                    break;
                }
                let mut slice = section_data[unpacked_index];
                if !palette.direct() {
                    slice = indices[slice as usize] as u16;
                }
                cell = (cell & !(mask << bit_index)) | ((slice as u64 & mask) << bit_index);
                unpacked_index += 1;
                bit_index += bits;
            }
            paletted_data.packed_long_array[cell_index] = cell;
        }

        Self {
            data: Data::Paletted(paletted_data),
        }
    }

    pub fn unpack(&self) -> UnpackedData<UNPACKED_SIZE> {
        match &self.data {
            Data::Single(atom) => [*atom; UNPACKED_SIZE],
            Data::Paletted(paletted_data) => {
                let mut unpacked = [0u16; UNPACKED_SIZE];
                let palette = &paletted_data.palette;
                let bits = palette.bits_per_entry as u8;
                let mask = (1u64 << bits) - 1;
                let mut unpacked_index = 0;

                for &cell in &paletted_data.packed_long_array {
                    let mut bit_index = 0u8;
                    while bit_index < 64 {
                        if unpacked_index >= UNPACKED_SIZE {
                            break;
                        }
                        let mut slice = (cell >> bit_index) & mask;
                        if !palette.direct() {
                            slice = palette.palette[slice as usize] as u64;
                        }
                        unpacked[unpacked_index] = slice as u16;
                        unpacked_index += 1;
                        bit_index += bits;
                    }
                }
                unpacked
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedSnapshot<const UNPACKED_SIZE: usize> {
    pub data: PackedData<UNPACKED_SIZE>,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackedDeltaData<const UNPACKED_SIZE: usize> {
    pub reverse_deltas: Vec<PackedSnapshot<UNPACKED_SIZE>>,
}

impl<const UNPACKED_SIZE: usize> PackedDeltaData<UNPACKED_SIZE> {
    pub fn latest_snapshot(&self) -> Option<&PackedSnapshot<UNPACKED_SIZE>> {
        self.delta(0)
    }

    pub fn delta(&self, delta_index: usize) -> Option<&PackedSnapshot<UNPACKED_SIZE>> {
        self.reverse_deltas.get(delta_index)
    }

    pub fn snapshot_from(&self, timestamp: i64) -> Option<UnpackedData<UNPACKED_SIZE>> {
        let nearest = find_nearest_timestamp(&self.reverse_deltas, |s| s.timestamp, timestamp);
        self.snapshot_before(nearest)
    }

    pub fn snapshot_before(&self, timestamp: i64) -> Option<UnpackedData<UNPACKED_SIZE>> {
        let latest_packed = self.latest_snapshot()?;
        let mut latest_unpacked = latest_packed.data.unpack();

        if timestamp >= latest_packed.timestamp {
            return Some(latest_unpacked);
        }

        for delta in self.reverse_deltas.iter().skip(1) {
            let unpacked = delta.data.unpack();
            for j in 0..UNPACKED_SIZE {
                let state = unpacked[j];
                if state != STATE_UNCHANGED {
                    latest_unpacked[j] = state;
                }
            }
            if timestamp >= delta.timestamp {
                break;
            }
        }
        Some(latest_unpacked)
    }

    pub fn insert_snapshot(
        &mut self,
        new_snapshot: PackedSnapshot<UNPACKED_SIZE>,
    ) -> Result<usize, DeltaInsertionStatus> {
        if let Some(latest) = self.latest_snapshot() {
            if new_snapshot.timestamp <= latest.timestamp {
                return Err(DeltaInsertionStatus::SnapshotOlderThanLatest);
            }

            let previous_unpacked = latest.data.unpack();
            let new_unpacked = new_snapshot.data.unpack();
            let mut delta_snapshot_builder = [0u16; UNPACKED_SIZE];
            let mut changes = 0;

            for i in 0..UNPACKED_SIZE {
                let previous = previous_unpacked[i];
                let changed = new_unpacked[i] != previous;
                delta_snapshot_builder[i] = if changed { previous } else { STATE_UNCHANGED };
                if changed {
                    changes += 1;
                }
            }

            if changes == 0 {
                return Err(DeltaInsertionStatus::NoChangesMade);
            }

            let delta_snapshot = PackedSnapshot {
                data: PackedData::pack(&delta_snapshot_builder),
                timestamp: latest.timestamp,
            };

            self.reverse_deltas.remove(0);
            self.reverse_deltas.insert(0, delta_snapshot);
            self.reverse_deltas.insert(0, new_snapshot);

            Ok(changes)
        } else {
            self.reverse_deltas.push(new_snapshot);
            Ok(UNPACKED_SIZE)
        }
    }
}
