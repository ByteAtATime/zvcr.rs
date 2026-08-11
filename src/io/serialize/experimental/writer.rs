use super::File;
use super::coder::context::ContextCodec;
use crate::definitions::*;
use crate::io::compression::*;
use crate::io::file_location::{EXTENSION, RegionLocation};
use crate::io::serialize::context::Context;
use crate::io::serialize::primitives::*;
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PackedSnapshot};
use crate::region::palette::{DIRECT_PALETTE, Palette, bits_per_entry};
use crate::region::segment::*;
use crate::region::segment_info::*;
use crate::region::tile_entities::*;
use crate::version::*;
use ahash::AHashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub(crate) type PaletteTable = AHashMap<Palette, usize>;

const BITPLANE_THRESHOLD: usize = 4;

pub(crate) struct WriteHandle {
    pub(crate) compression_level: i32,
    pub(crate) ctx: Context,
    pub(crate) data: Vec<u8>,
    block_palette_table: PaletteTable,
    biome_palette_table: PaletteTable,
    plane_scratch: Vec<u8>,
    encode_scratch: Vec<u8>,
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
            block_palette_table: AHashMap::new(),
            biome_palette_table: AHashMap::new(),
            plane_scratch: Vec::new(),
            encode_scratch: Vec::new(),
        }
    }

    pub(crate) fn serialize_palette_table(table: &PaletteTable, buf: &mut Vec<u8>) {
        let mut ordered = vec![DIRECT_PALETTE.clone(); table.len()];
        for (palette, &index) in table {
            ordered[index] = palette.clone();
        }

        let entries: Vec<Vec<u16>> = ordered
            .iter()
            .map(|p| p.palette.iter().copied().collect())
            .collect();
        let encoded = super::coder::palette::encode_palette_table(&entries);
        put_u32_le(buf, encoded.len() as u32);
        buf.extend_from_slice(&encoded);
    }

    pub(crate) fn record_palette_index(&mut self, palette: &Palette, is_block: bool) {
        let table = if is_block {
            &mut self.block_palette_table
        } else {
            &mut self.biome_palette_table
        };
        if palette.direct() {
            put_u32_le(&mut self.data, u32::MAX);
            return;
        }
        let idx = if let Some(&existing) = table.get(palette) {
            existing
        } else {
            let next_index = table.len();
            table.insert(palette.clone(), next_index);
            next_index
        };
        put_u32_le(&mut self.data, idx as u32);
    }

    pub(crate) fn serialize_snapshot_header<const UNPACKED_SIZE: usize>(
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
                self.record_palette_index(&paletted.palette, is_block);
                if !paletted.palette.direct() {
                    let bits = paletted.palette.bits_per_entry as u8;
                    let remapped =
                        super::bitplane::remap_to_popcount(&paletted.packed_long_array, bits);
                    put_u8(
                        &mut self.data,
                        super::bitplane::compute_plane_mask(&remapped, bits),
                    );
                }
            }
        }
    }

    pub(crate) fn serialize_snapshot_body<const UNPACKED_SIZE: usize>(
        &mut self,
        snapshot: &PackedSnapshot<UNPACKED_SIZE>,
    ) {
        let Data::Paletted(paletted) = &snapshot.data.data else {
            return;
        };
        if paletted.palette.direct() {
            put_u64_le_slice(&mut self.data, &paletted.packed_long_array);
            return;
        }
        let bits = paletted.palette.bits_per_entry as u8;
        let remapped = super::bitplane::remap_to_popcount(&paletted.packed_long_array, bits);
        let mask = super::bitplane::compute_plane_mask(&remapped, bits);
        let plane_bytes = UNPACKED_SIZE / 8;
        let byte_len = mask.count_ones() as usize * plane_bytes;

        self.plane_scratch.clear();
        self.plane_scratch.resize(byte_len, 0);
        super::bitplane::pack_bitplanes_into::<UNPACKED_SIZE>(
            &remapped,
            bits,
            mask,
            &mut self.plane_scratch[..byte_len],
        );

        self.encode_scratch.clear();
        super::rle::encode(&self.plane_scratch[..byte_len], &mut self.encode_scratch);
        put_u16_le(&mut self.data, self.encode_scratch.len() as u16);
        put_bytes(&mut self.data, &self.encode_scratch);
    }

    pub(crate) fn serialize_block_header_j0(
        &mut self,
        snapshot: &PackedSnapshot<SECTION_SIZE_BLOCKS>,
        counts: &mut [u32],
        unique: &mut Vec<u16>,
    ) {
        put_u64_le(&mut self.data, snapshot.timestamp as u64);
        let blocks = snapshot.data.unpack();

        unique.clear();
        for &atom in blocks.iter() {
            if counts[atom as usize] == 0 {
                unique.push(atom);
            }
            counts[atom as usize] += 1;
        }
        unique
            .sort_unstable_by(|&a, &b| counts[b as usize].cmp(&counts[a as usize]).then(a.cmp(&b)));

        if unique.len() <= 1 {
            put_u8(&mut self.data, 0);
            put_u16_le(&mut self.data, unique.first().copied().unwrap_or(0));
        } else if unique.len() <= BITPLANE_THRESHOLD {
            put_u8(&mut self.data, 1);
            let Data::Paletted(paletted) = &snapshot.data.data else {
                unreachable!()
            };
            self.record_palette_index(&paletted.palette, true);
            if !paletted.palette.direct() {
                let bits = paletted.palette.bits_per_entry as u8;
                let remapped =
                    super::bitplane::remap_to_popcount(&paletted.packed_long_array, bits);
                put_u8(
                    &mut self.data,
                    super::bitplane::compute_plane_mask(&remapped, bits),
                );
            }
        } else {
            put_u8(&mut self.data, 11);
            let palette = Palette {
                palette: unique.clone().into(),
                bits_per_entry: bits_per_entry(unique.len()),
            };
            self.record_palette_index(&palette, true);
        }

        for &atom in unique.iter() {
            counts[atom as usize] = 0;
        }
    }

    pub(crate) fn serialize_block_body_j0(
        &mut self,
        snapshot: &PackedSnapshot<SECTION_SIZE_BLOCKS>,
        codec: &mut ContextCodec,
        cx: usize,
        cz: usize,
        sec_idx: usize,
        counts: &mut [u32],
        unique: &mut Vec<u16>,
        val_to_local: &mut [u16],
    ) {
        let blocks = snapshot.data.unpack();

        unique.clear();
        for &atom in blocks.iter() {
            if counts[atom as usize] == 0 {
                unique.push(atom);
            }
            counts[atom as usize] += 1;
        }
        unique
            .sort_unstable_by(|&a, &b| counts[b as usize].cmp(&counts[a as usize]).then(a.cmp(&b)));

        codec.write_recon(&blocks, cx, cz, sec_idx);

        if unique.len() <= 1 {
            for &atom in unique.iter() {
                counts[atom as usize] = 0;
            }
            return;
        }

        if unique.len() <= BITPLANE_THRESHOLD {
            self.serialize_snapshot_body(snapshot);
            for &atom in unique.iter() {
                counts[atom as usize] = 0;
            }
            return;
        }

        for (local, &atom) in unique.iter().enumerate() {
            val_to_local[atom as usize] = local as u16;
        }

        let encoded = codec.encode_section(&blocks, val_to_local, cx, cz, sec_idx);
        put_u32_le(&mut self.data, encoded.len() as u32);
        put_bytes(&mut self.data, &encoded);

        for &atom in unique.iter() {
            counts[atom as usize] = 0;
            val_to_local[atom as usize] = 0;
        }
    }

    pub(crate) fn serialize_column_headers<const UNPACKED_SIZE: usize>(
        &mut self,
        sections: &[&PackedDeltaData<UNPACKED_SIZE>],
        is_block: bool,
    ) {
        for section in sections {
            put_u64_le(&mut self.data, section.reverse_deltas.len() as u64);
        }
        for section in sections {
            for snapshot in &section.reverse_deltas {
                self.serialize_snapshot_header(snapshot, is_block);
            }
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

    pub(crate) fn serialize_region(&mut self, region: &Region) -> Result<(), String> {
        let mut inner_handle = WriteHandle::new(self.ctx.protocol_version, self.compression_level);
        inner_handle.ctx = self.ctx.clone();
        let section_count = inner_handle.ctx.section_count;

        let present: Vec<&Arc<Segment>> = region.segments.iter().flatten().collect();
        let present_indices: Vec<usize> = (0..SEGMENTS_PER_REGION)
            .filter(|&i| region.segments[i].is_some())
            .collect();

        for segment in &region.segments {
            put_u8(
                &mut inner_handle.data,
                if segment.is_some() { 1 } else { 0 },
            );
        }

        let block_columns: Vec<Vec<_>> = (0..section_count)
            .map(|y| {
                present
                    .iter()
                    .map(|segment| &segment.block_sections.sections[y])
                    .collect::<Vec<_>>()
            })
            .collect();
        let biome_columns: Vec<Vec<_>> = (0..section_count)
            .map(|y| {
                present
                    .iter()
                    .map(|segment| &segment.biome_sections.sections[y])
                    .collect::<Vec<_>>()
            })
            .collect();

        let mut counts = vec![0u32; 65536];
        let mut unique = Vec::new();

        for y in 0..section_count {
            for section in &block_columns[y] {
                put_u64_le(&mut inner_handle.data, section.reverse_deltas.len() as u64);
            }
            for section in &block_columns[y] {
                for (j, snapshot) in section.reverse_deltas.iter().enumerate() {
                    if j == 0 {
                        inner_handle.serialize_block_header_j0(snapshot, &mut counts, &mut unique);
                    } else {
                        inner_handle.serialize_snapshot_header(snapshot, true);
                    }
                }
            }
        }

        for column in &biome_columns {
            inner_handle.serialize_column_headers(column, false);
        }

        let mut codec = ContextCodec::new(section_count);
        codec.reset(section_count);
        let mut val_to_local = vec![0u16; 65536];

        for y in 0..section_count {
            for (k, section) in block_columns[y].iter().enumerate() {
                if let Some(snapshot) = section.reverse_deltas.first() {
                    let seg_idx = present_indices[k];
                    let cx = seg_idx / 32;
                    let cz = seg_idx % 32;
                    inner_handle.serialize_block_body_j0(
                        snapshot,
                        &mut codec,
                        cx,
                        cz,
                        y,
                        &mut counts,
                        &mut unique,
                        &mut val_to_local,
                    );
                }
            }
        }

        for y in 0..section_count {
            for section in &block_columns[y] {
                for snapshot in section.reverse_deltas.iter().skip(1) {
                    inner_handle.serialize_snapshot_body(snapshot);
                }
            }
        }

        for column in &biome_columns {
            for section in column {
                if let Some(snapshot) = section.reverse_deltas.first() {
                    inner_handle.serialize_snapshot_body(snapshot);
                }
            }
            for section in column {
                for snapshot in section.reverse_deltas.iter().skip(1) {
                    inner_handle.serialize_snapshot_body(snapshot);
                }
            }
        }

        for segment in &present {
            inner_handle.serialize_segment_info(&segment.info);
        }
        for segment in &present {
            inner_handle.serialize_tile_entities(&segment.tile_entities);
        }

        let mut palette_tables = Vec::new();
        Self::serialize_palette_table(&inner_handle.block_palette_table, &mut palette_tables);
        Self::serialize_palette_table(&inner_handle.biome_palette_table, &mut palette_tables);

        let compressed = compress_zstd_parts(
            &[&palette_tables, &inner_handle.data],
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
