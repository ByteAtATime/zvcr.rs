use super::File;
use super::transforms::palette::PaletteTable;
use crate::io::compression::*;
use crate::io::file_location::{EXTENSION, RegionLocation};
use crate::io::serialize::context::Context;
use crate::io::serialize::primitives::*;
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PackedSnapshot};
use crate::region::segment::*;
use crate::region::segment_info::*;
use crate::region::tile_entities::*;
use crate::version::*;
use std::fs;
use std::path::Path;

pub(crate) struct WriteHandle {
    pub(crate) compression_level: i32,
    pub(crate) ctx: Context,
    pub(crate) data: Vec<u8>,
    block_palette_table: PaletteTable,
    biome_palette_table: PaletteTable,
}

impl WriteHandle {
    pub(crate) fn new(protocol_version: u16, compression_level: i32) -> Self {
        Self {
            compression_level,
            ctx: Context {
                section_count: 0,
                protocol_version,
            },
            data: Vec::with_capacity(32 * 1024 * 1024),
            block_palette_table: PaletteTable::new(),
            biome_palette_table: PaletteTable::new(),
        }
    }

    fn serialize_packed_snapshot<const N: usize>(
        data: &mut Vec<u8>,
        snapshot: &PackedSnapshot<N>,
        palette_table: &PaletteTable,
    ) {
        put_u64_le(data, snapshot.timestamp as u64);
        match &snapshot.data.data {
            Data::Single(val) => {
                put_u8(data, 0);
                put_u16_le(data, *val);
            }
            Data::Paletted(paletted) => {
                put_u8(data, 1);
                let packed_u64_count = (paletted.packed_long_array.len() / 8) as u64;
                put_u64_le(data, packed_u64_count);
                data.extend_from_slice(&paletted.packed_long_array);
                put_u32_le(data, palette_table.index_for(&paletted.palette));
            }
        }
    }

    fn serialize_packed_delta_data<const N: usize>(
        data: &mut Vec<u8>,
        delta_data: &PackedDeltaData<N>,
        palette_table: &PaletteTable,
    ) {
        let snapshots = delta_data.snapshots();
        put_u64_le(data, snapshots.len() as u64);
        for snapshot in snapshots {
            Self::serialize_packed_snapshot(data, snapshot, palette_table);
        }
    }

    fn record_packed_delta_data<const N: usize>(
        delta_data: &PackedDeltaData<N>,
        palette_table: &mut PaletteTable,
    ) {
        for snapshot in delta_data.snapshots() {
            if let Data::Paletted(paletted) = &snapshot.data.data {
                palette_table.record(&paletted.palette);
            }
        }
    }

    fn record_segment_palettes(&mut self, segment: &Segment) {
        for section in segment.block_sections.active() {
            Self::record_packed_delta_data(section, &mut self.block_palette_table);
        }
        for section in segment.biome_sections.active() {
            Self::record_packed_delta_data(section, &mut self.biome_palette_table);
        }
    }

    fn serialize_segment_state(data: &mut Vec<u8>, state: &SegmentState) {
        put_u8(data, state.state_type as u8);
        put_u64_le(data, state.timestamp as u64);
    }

    fn serialize_segment_info(data: &mut Vec<u8>, info: &SegmentInfo) {
        put_u64_le(data, info.reverse_deltas.len() as u64);
        for state in &info.reverse_deltas {
            Self::serialize_segment_state(data, state);
        }
    }

    fn serialize_tile_entities(data: &mut Vec<u8>, tile_entities: &DeltaTileEntityData) {
        put_u64_le(data, tile_entities.reverse_deltas.len() as u64);
        for list_delta in &tile_entities.reverse_deltas {
            put_u64_le(data, list_delta.timestamp as u64);
            put_u64_le(data, list_delta.deltas.len() as u64);

            let mut sorted_entries: Vec<_> = list_delta.deltas.iter().collect();
            sorted_entries.sort_unstable_by_key(|(pos, _)| pos.packed());
            for (pos, delta) in sorted_entries {
                put_u32_le(data, pos.packed());
                match delta {
                    TileEntityDelta::Erase => {
                        put_u8(data, 0);
                    }
                    TileEntityDelta::Put(te) => {
                        put_u8(data, 1);
                        put_u32_le(data, te.tile_type);
                        put_u64_le(data, te.nbt.len() as u64);
                        put_bytes(data, &te.nbt);
                    }
                }
            }
        }
    }

    pub(crate) fn serialize_region(&mut self, region: &Region) -> Result<(), String> {
        let mut inner_handle = WriteHandle::new(self.ctx.protocol_version, self.compression_level);
        inner_handle.ctx = self.ctx.clone();

        for segment in &region.segments {
            put_u8(
                &mut inner_handle.data,
                if segment.is_some() { 1 } else { 0 },
            );
        }

        for segment in region.segments.iter().flatten() {
            inner_handle.record_segment_palettes(segment);
        }
        inner_handle.block_palette_table.finalize();
        inner_handle.biome_palette_table.finalize();

        let present_count = region.segments.iter().flatten().count();
        let sections = self.ctx.section_count;
        let mut block_data = Vec::with_capacity(present_count * sections * 512);
        let mut biome_data = Vec::with_capacity(present_count * sections * 128);
        let mut info_data = Vec::with_capacity(present_count * 64);
        let mut tile_entity_data = Vec::with_capacity(present_count * 256);

        for segment in region.segments.iter().flatten() {
            for section in segment.block_sections.active() {
                Self::serialize_packed_delta_data(
                    &mut block_data,
                    section,
                    &inner_handle.block_palette_table,
                );
            }
        }
        for segment in region.segments.iter().flatten() {
            for section in segment.biome_sections.active() {
                Self::serialize_packed_delta_data(
                    &mut biome_data,
                    section,
                    &inner_handle.biome_palette_table,
                );
            }
        }
        for segment in region.segments.iter().flatten() {
            Self::serialize_segment_info(&mut info_data, &segment.info);
        }
        for segment in region.segments.iter().flatten() {
            Self::serialize_tile_entities(&mut tile_entity_data, &segment.tile_entities);
        }

        let mut palette_tables = Vec::new();
        inner_handle
            .block_palette_table
            .serialize(&mut palette_tables);
        inner_handle
            .biome_palette_table
            .serialize(&mut palette_tables);

        let compressed = compress_zstd_parts(
            &[
                &palette_tables,
                &inner_handle.data,
                &block_data,
                &biome_data,
                &info_data,
                &tile_entity_data,
            ],
            self.compression_level,
        )?;
        put_bytes(&mut self.data, &compressed);
        Ok(())
    }

    pub(crate) fn serialize_file(&mut self, file: &File) -> Result<(), String> {
        self.ctx.initialize_section_count(file.dimension_type);
        put_bytes(&mut self.data, EXTENSION.as_bytes());
        put_u8(&mut self.data, ZVCR3D_LATEST_VERSION as u8);
        put_u8(&mut self.data, file.dimension_type as u8);
        put_u16_le(&mut self.data, file.protocol_version);

        self.serialize_region(&file.region)
    }
}

pub(crate) fn serialize_file_to_vec(
    file: &File,
    compression_level: i32,
) -> Result<Vec<u8>, String> {
    let mut handle = WriteHandle::new(file.protocol_version, compression_level);
    handle.serialize_file(file)?;
    Ok(handle.data)
}

#[allow(dead_code)]
pub(crate) fn write_file(
    file: &File,
    filepath: &Path,
    compression_level: i32,
) -> Result<usize, String> {
    let bytes = serialize_file_to_vec(file, compression_level)?;
    fs::write(filepath, &bytes).map_err(|e| format!("Failed to write file to disk: {e}"))?;
    Ok(bytes.len())
}

#[allow(dead_code)]
pub(crate) fn write_file_at(
    file: &File,
    parent_directory: &Path,
    location: &RegionLocation,
    compression_level: i32,
) -> Result<usize, String> {
    let target_dir = location.directory(parent_directory);
    fs::create_dir_all(&target_dir).map_err(|e| format!("Failed to create directories: {e}"))?;

    write_file(
        file,
        &location.file_path(parent_directory),
        compression_level,
    )
}
