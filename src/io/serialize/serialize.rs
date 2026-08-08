use crate::definitions::*;
use crate::dimension::DimensionType;
use crate::io::compression::*;
use crate::io::file_location::{EXTENSION, RegionLocation};
use crate::io::serialize::context::Context;
use crate::region::paletted_delta_data::*;
use crate::region::segment::*;
use crate::region::segment_info::*;
use crate::region::tile_entities::*;
use crate::version::*;
use ahash::AHashMap;
use std::fs;
use std::path::Path;

fn put_u8(buf: &mut Vec<u8>, v: u8) {
    buf.push(v);
}

fn put_u16_le(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u32_le(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u64_le(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(buf: &mut Vec<u8>, v: &[u8]) {
    buf.extend_from_slice(v);
}

pub const DEFAULT_PROTOCOL_VERSION: u16 = 769;

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
            protocol_version: DEFAULT_PROTOCOL_VERSION,
            dimension_type: DimensionType::Overworld,
            region: Region::new(DEFAULT_PROTOCOL_VERSION),
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

    pub fn serialize_palette_table(table: &PaletteTable, buf: &mut Vec<u8>) {
        let mut ordered = vec![DIRECT_PALETTE; table.len()];
        for (palette, &index) in table {
            ordered[index] = palette.clone();
        }

        put_u32_le(buf, ordered.len() as u32);
        for palette in &ordered {
            let len = palette.length();
            if palette.direct() || len == 1 {
                continue;
            }
            put_u16_le(buf, len as u16);
            for &atom in &palette.palette {
                put_u16_le(buf, atom);
            }
        }
    }

    pub fn serialize_packed_snapshot<const UNPACKED_SIZE: usize>(
        &mut self,
        snapshot: &PackedSnapshot<UNPACKED_SIZE>,
        is_block: bool,
    ) {
        put_u64_le(&mut self.data, snapshot.timestamp as u64);
        match &snapshot.data.data {
            Data::Single(val) => {
                put_u8(&mut self.data, 0);
                put_u16_le(&mut self.data, *val);
            }
            Data::Paletted(paletted) => {
                put_u8(&mut self.data, 1);
                let packed_len = paletted.packed_long_array.len();
                put_u64_le(&mut self.data, packed_len as u64);
                for &val in &paletted.packed_long_array {
                    put_u64_le(&mut self.data, val);
                }

                let palette_table = if is_block {
                    &mut self.block_palette_table
                } else {
                    &mut self.biome_palette_table
                };

                if paletted.palette.direct() {
                    put_u32_le(&mut self.data, u32::MAX);
                } else {
                    let idx = if let Some(&existing) = palette_table.get(&paletted.palette) {
                        existing
                    } else {
                        let next_index = palette_table.len();
                        palette_table.insert(paletted.palette.clone(), next_index);
                        next_index
                    };
                    put_u32_le(&mut self.data, idx as u32);
                }
            }
        }
    }

    pub fn serialize_packed_delta_data<const UNPACKED_SIZE: usize>(
        &mut self,
        delta_data: &PackedDeltaData<UNPACKED_SIZE>,
        is_block: bool,
    ) {
        put_u64_le(&mut self.data, delta_data.reverse_deltas.len() as u64);
        for snapshot in &delta_data.reverse_deltas {
            self.serialize_packed_snapshot(snapshot, is_block);
        }
    }

    pub fn serialize_segment_state(&mut self, state: &SegmentState) {
        put_u8(&mut self.data, state.state_type as u8);
        put_u64_le(&mut self.data, state.timestamp as u64);
    }

    pub fn serialize_segment_info(&mut self, info: &SegmentInfo) {
        put_u64_le(&mut self.data, info.reverse_deltas.len() as u64);
        for state in &info.reverse_deltas {
            self.serialize_segment_state(state);
        }
    }

    pub fn serialize_tile_entities(&mut self, tile_entities: &DeltaTileEntityData) {
        put_u64_le(&mut self.data, tile_entities.reverse_deltas.len() as u64);
        for list_delta in &tile_entities.reverse_deltas {
            put_u64_le(&mut self.data, list_delta.timestamp as u64);
            put_u64_le(&mut self.data, list_delta.deltas.len() as u64);

            let mut sorted_positions: Vec<_> = list_delta.deltas.keys().cloned().collect();
            sorted_positions.sort_by_key(|pos| pos.packed());

            for pos in sorted_positions {
                let delta = &list_delta.deltas[&pos];
                put_u32_le(&mut self.data, pos.packed());
                match delta {
                    TileEntityDelta::Erase => {
                        put_u8(&mut self.data, 0);
                    }
                    TileEntityDelta::Put(te) => {
                        put_u8(&mut self.data, 1);
                        put_u32_le(&mut self.data, te.tile_type);
                        put_u64_le(&mut self.data, te.nbt.len() as u64);
                        put_bytes(&mut self.data, &te.nbt);
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
        let mut inner_handle = WriteHandle::new(
            self.ctx.protocol_version,
            self.compression_level,
            self.compression_threads,
        );
        inner_handle.ctx = self.ctx.clone();

        for i in 0..SEGMENTS_PER_REGION {
            if let Some(ref seg) = region.segments[i] {
                put_u8(&mut inner_handle.data, 1);
                inner_handle.serialize_segment(seg);
            } else {
                put_u8(&mut inner_handle.data, 0);
            }
        }

        let mut body = Vec::with_capacity(inner_handle.data.len());
        Self::serialize_palette_table(&inner_handle.block_palette_table, &mut body);
        Self::serialize_palette_table(&inner_handle.biome_palette_table, &mut body);
        put_bytes(&mut body, &inner_handle.data);

        let compressed = compress_zstd(&body, self.compression_level, self.compression_threads)?;
        put_bytes(&mut self.data, &compressed);
        Ok(())
    }

    pub fn serialize_file(&mut self, file: &File) -> Result<(), String> {
        self.ctx.initialize_section_count(file.dimension_type);
        put_bytes(&mut self.data, EXTENSION.as_bytes());
        put_u8(&mut self.data, ZVCR3D_LATEST_VERSION as u8);
        put_u8(&mut self.data, file.dimension_type as u8);
        put_u16_le(&mut self.data, file.protocol_version);

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
