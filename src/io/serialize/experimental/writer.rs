use super::File;
use crate::io::compression::*;
use crate::io::file_location::{EXTENSION, RegionLocation};
use crate::io::serialize::context::Context;
use crate::io::serialize::primitives::*;
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::PackedSnapshot;
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
        }
    }

    fn put_atoms<const N: usize>(&mut self, atoms: &[u16; N]) {
        let byte_len = N * std::mem::size_of::<u16>();
        self.data.reserve(byte_len);
        #[cfg(target_endian = "little")]
        self.data.extend_from_slice(unsafe {
            std::slice::from_raw_parts(atoms.as_ptr() as *const u8, byte_len)
        });
        #[cfg(not(target_endian = "little"))]
        for &atom in atoms {
            put_u16_le(&mut self.data, atom);
        }
    }

    pub(crate) fn serialize_snapshot<const N: usize>(&mut self, snapshot: &PackedSnapshot<N>) {
        put_u64_le(&mut self.data, snapshot.timestamp as u64);
        let atoms = snapshot.data.unpack();
        self.put_atoms(&atoms);
    }

    pub(crate) fn serialize_packed_delta_data<const N: usize>(
        &mut self,
        delta_data: &PackedDeltaData<N>,
    ) {
        put_u64_le(&mut self.data, delta_data.reverse_deltas.len() as u64);
        for snapshot in &delta_data.reverse_deltas {
            self.serialize_snapshot(snapshot);
        }
    }

    pub(crate) fn serialize_segment_state(&mut self, state: &SegmentState) {
        put_u8(&mut self.data, state.state_type as u8);
        put_u64_le(&mut self.data, state.timestamp as u64);
    }

    pub(crate) fn serialize_segment_info(&mut self, info: &SegmentInfo) {
        put_u64_le(&mut self.data, info.reverse_deltas.len() as u64);
        for state in &info.reverse_deltas {
            self.serialize_segment_state(state);
        }
    }

    pub(crate) fn serialize_tile_entities(&mut self, tile_entities: &DeltaTileEntityData) {
        put_u64_le(&mut self.data, tile_entities.reverse_deltas.len() as u64);
        for list_delta in &tile_entities.reverse_deltas {
            put_u64_le(&mut self.data, list_delta.timestamp as u64);
            put_u64_le(&mut self.data, list_delta.deltas.len() as u64);

            let mut sorted_entries: Vec<_> = list_delta.deltas.iter().collect();
            sorted_entries.sort_unstable_by_key(|(pos, _)| pos.packed());
            for (pos, delta) in sorted_entries {
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

    pub(crate) fn serialize_segment(&mut self, segment: &Segment) {
        for section in segment.block_sections.active() {
            self.serialize_packed_delta_data(section);
        }
        for section in segment.biome_sections.active() {
            self.serialize_packed_delta_data(section);
        }
        self.serialize_segment_info(&segment.info);
        self.serialize_tile_entities(&segment.tile_entities);
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
            inner_handle.serialize_segment(segment);
        }

        let compressed = compress_zstd_parts(&[&inner_handle.data], self.compression_level)?;
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
