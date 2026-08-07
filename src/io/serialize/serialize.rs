use crate::definitions::*;
use crate::dimension::DimensionType;
use crate::io::compression::*;
use crate::io::file_location::RegionLocation;
use crate::io::serialize::context::Context;
use crate::region::paletted_delta_data::*;
use crate::region::segment::*;
use crate::region::segment_info::*;
use crate::region::tile_entities::*;
use crate::version::*;
use ahash::AHashMap;
use byteorder::{LittleEndian, WriteBytesExt};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct File {
    pub version: Version,
    pub protocol_version: u16,
    pub dimension_type: DimensionType,
    pub region: Region,
}

impl Default for File {
    fn default() -> Self {
        Self {
            version: ZVCR3D_LATEST_VERSION,
            protocol_version: 769,
            dimension_type: DimensionType::Overworld,
            region: Region::new(769),
        }
    }
}

pub type PaletteTable = AHashMap<Palette, usize>;

pub struct WriteHandle {
    pub compression_level: i32,
    pub compression_threads: u32,
    pub ctx: Context,
    pub data: Vec<u8>,
    block_palette_table: PaletteTable,
    biome_palette_table: PaletteTable,
}

impl WriteHandle {
    pub fn new(protocol_version: u16, compression_level: i32, compression_threads: u32) -> Self {
        Self {
            compression_level,
            compression_threads,
            ctx: Context {
                section_count: 0,
                protocol_version,
            },
            data: Vec::with_capacity(32 * 1024 * 1024),
            block_palette_table: AHashMap::new(),
            biome_palette_table: AHashMap::new(),
        }
    }

    pub fn serialize_palette_table(&mut self, table: &PaletteTable) {
        let mut ordered = vec![DIRECT_PALETTE; table.len()];
        for (palette, &index) in table {
            ordered[index] = palette.clone();
        }

        self.data
            .write_u32::<LittleEndian>(ordered.len() as u32)
            .unwrap();
        for palette in &ordered {
            let len = palette.length();
            if palette.direct() || len == 1 {
                continue;
            }
            self.data.write_u16::<LittleEndian>(len as u16).unwrap();
            for &atom in &palette.palette {
                self.data.write_u16::<LittleEndian>(atom).unwrap();
            }
        }
    }

    pub fn serialize_packed_snapshot<const UNPACKED_SIZE: usize>(
        &mut self,
        snapshot: &PackedSnapshot<UNPACKED_SIZE>,
        is_block: bool,
    ) {
        self.data
            .write_u64::<LittleEndian>(snapshot.timestamp as u64)
            .unwrap();
        match &snapshot.data.data {
            Data::Single(val) => {
                self.data.write_u8(0).unwrap();
                self.data.write_u16::<LittleEndian>(*val).unwrap();
            }
            Data::Paletted(paletted) => {
                self.data.write_u8(1).unwrap();
                let packed_len = paletted.packed_long_array.len();
                self.data
                    .write_u64::<LittleEndian>(packed_len as u64)
                    .unwrap();
                for &val in &paletted.packed_long_array {
                    self.data.write_u64::<LittleEndian>(val).unwrap();
                }

                let palette_table = if is_block {
                    &mut self.block_palette_table
                } else {
                    &mut self.biome_palette_table
                };

                if paletted.palette.direct() {
                    self.data.write_u32::<LittleEndian>(u32::MAX).unwrap();
                } else {
                    let next_index = palette_table.len();
                    let idx = *palette_table
                        .entry(paletted.palette.clone())
                        .or_insert(next_index);
                    self.data.write_u32::<LittleEndian>(idx as u32).unwrap();
                }
            }
        }
    }

    pub fn serialize_packed_delta_data<const UNPACKED_SIZE: usize>(
        &mut self,
        delta_data: &PackedDeltaData<UNPACKED_SIZE>,
        is_block: bool,
    ) {
        self.data
            .write_u64::<LittleEndian>(delta_data.reverse_deltas.len() as u64)
            .unwrap();
        for snapshot in &delta_data.reverse_deltas {
            self.serialize_packed_snapshot(snapshot, is_block);
        }
    }

    pub fn serialize_segment_state(&mut self, state: &SegmentState) {
        self.data.write_u8(state.state_type as u8).unwrap();
        self.data
            .write_u64::<LittleEndian>(state.timestamp as u64)
            .unwrap();
    }

    pub fn serialize_segment_info(&mut self, info: &SegmentInfo) {
        self.data
            .write_u64::<LittleEndian>(info.segment_states.len() as u64)
            .unwrap();
        for state in &info.segment_states {
            self.serialize_segment_state(state);
        }
    }

    pub fn serialize_tile_entities(&mut self, tile_entities: &DeltaTileEntityData) {
        self.data
            .write_u64::<LittleEndian>(tile_entities.reverse_deltas.len() as u64)
            .unwrap();
        for list_delta in &tile_entities.reverse_deltas {
            self.data
                .write_u64::<LittleEndian>(list_delta.timestamp as u64)
                .unwrap();
            self.data
                .write_u64::<LittleEndian>(list_delta.deltas.len() as u64)
                .unwrap();

            let mut sorted_positions: Vec<_> = list_delta.deltas.keys().cloned().collect();
            sorted_positions.sort_by_key(|pos| pos.packed());

            for pos in sorted_positions {
                let delta = &list_delta.deltas[&pos];
                self.data.write_u32::<LittleEndian>(pos.packed()).unwrap();
                match delta {
                    TileEntityDelta::Erase => {
                        self.data.write_u8(0).unwrap();
                    }
                    TileEntityDelta::Put(te) => {
                        self.data.write_u8(1).unwrap();
                        self.data.write_u32::<LittleEndian>(te.tile_type).unwrap();
                        self.data
                            .write_u64::<LittleEndian>(te.nbt.len() as u64)
                            .unwrap();
                        self.data.write_all(&te.nbt).unwrap();
                    }
                }
            }
        }
    }

    pub fn serialize_segment(&mut self, segment: &Segment) {
        for i in 0..self.ctx.section_count {
            self.serialize_packed_delta_data(&segment.block_sections.sections[i], true);
        }
        for i in 0..self.ctx.section_count {
            self.serialize_packed_delta_data(&segment.biome_sections.sections[i], false);
        }
        self.serialize_segment_info(&segment.info);
        self.serialize_tile_entities(&segment.tile_entities);
    }

    pub fn serialize_region(&mut self, region: &Region) -> Result<(), String> {
        // Bytes already in `self.data` form the uncompressed file header
        // ("zvcr3d" + version + dimension + protocol version). The header must
        // remain uncompressed; only the region body that follows is zstd-
        // compressed. Capture the boundary so we never compress the header.
        let header_len = self.data.len();

        let mut inner_handle = WriteHandle::new(
            self.ctx.protocol_version,
            self.compression_level,
            self.compression_threads,
        );
        inner_handle.ctx = self.ctx.clone();

        for i in 0..SEGMENTS_PER_REGION {
            if let Some(ref seg) = region.segments[i] {
                inner_handle.data.write_u8(1).unwrap();
                inner_handle.serialize_segment(seg);
            } else {
                inner_handle.data.write_u8(0).unwrap();
            }
        }

        self.serialize_palette_table(&inner_handle.block_palette_table);
        self.serialize_palette_table(&inner_handle.biome_palette_table);
        self.data.write_all(&inner_handle.data).unwrap();

        // Compress only the region body, leaving the header untouched.
        let body = self.data[header_len..].to_vec();
        let compressed =
            compress_zstd(&body, self.compression_level, self.compression_threads)?;

        self.data.truncate(header_len);
        self.data.extend_from_slice(&compressed);
        Ok(())
    }

    pub fn serialize_file(&mut self, file: &File) -> Result<(), String> {
        self.ctx.initialize_section_count(file.dimension_type);
        self.data.write_all(b"zvcr3d").unwrap();
        self.data.write_u8(ZVCR3D_LATEST_VERSION as u8).unwrap();
        self.data.write_u8(file.dimension_type as u8).unwrap();
        self.data
            .write_u16::<LittleEndian>(file.protocol_version)
            .unwrap();

        self.serialize_region(&file.region)
    }
}

pub fn write_file(
    file: &File,
    filepath: &Path,
    compression_level: i32,
    compression_threads: u32,
) -> Result<usize, String> {
    let mut handle = WriteHandle::new(
        file.protocol_version,
        compression_level,
        compression_threads,
    );
    handle.serialize_file(file)?;

    fs::write(filepath, &handle.data).map_err(|e| format!("Failed to write file to disk: {e}"))?;

    Ok(handle.data.len())
}

pub fn write_file_at(
    file: &File,
    parent_directory: &Path,
    location: &RegionLocation,
    compression_level: i32,
    compression_threads: u32,
) -> Result<usize, String> {
    let target_dir = location.directory(parent_directory);
    fs::create_dir_all(&target_dir).map_err(|e| format!("Failed to create directories: {e}"))?;

    write_file(
        file,
        &location.file_path(parent_directory),
        compression_level,
        compression_threads,
    )
}
