use crate::definitions::*;
use crate::dimension::DimensionType;
use crate::io::compression::decompress_zstd;
use crate::io::file_location::{EXTENSION, RegionLocation};
use super::File;
use crate::io::serialize::context::Context;
use crate::io::serialize::error::*;
use crate::io::serialize::primitives::ByteCursor;
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PackedData, PackedSnapshot, PalettedData};
use crate::region::palette::{DIRECT_PALETTE, MAX_INDIRECT_PALETTE_SIZE, Palette, bits_per_entry};
use crate::region::segment::*;
use crate::region::segment_info::*;
use crate::region::tile_entities::*;
use crate::version::*;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::ops::{Deref, DerefMut};

pub(crate) struct ReadHandle {
    pub(crate) ctx: Context,
    cursor: ByteCursor,
    max_deltas: usize,
    plane_scratch: Vec<u8>,
}

impl Deref for ReadHandle {
    type Target = ByteCursor;
    fn deref(&self) -> &Self::Target {
        &self.cursor
    }
}

impl DerefMut for ReadHandle {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.cursor
    }
}

impl ReadHandle {
    pub(crate) fn new(data: Vec<u8>, max_deltas: usize) -> Self {
        Self {
            ctx: Context::default(),
            cursor: ByteCursor::new(data),
            max_deltas,
            plane_scratch: Vec::new(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn offset(&self) -> usize {
        self.pos
    }

    pub(crate) fn validate_file_prefix(&mut self, prefix: &str) -> Result<(), ReadError> {
        let mut buf = vec![0u8; prefix.len()];
        self.read_exact(&mut buf)?;
        if buf != prefix.as_bytes() {
            return Err(ReadError::HeaderMismatch);
        }
        Ok(())
    }

    pub(crate) fn deserialize_version(&mut self, latest: Version) -> Result<Version, ReadError> {
        let ver_num = self.read_u8()?;
        if ver_num > latest as u8 {
            return Err(ReadError::InvalidVersion(ver_num));
        }
        Version::from_u8(ver_num).ok_or(ReadError::InvalidVersion(ver_num))
    }

    pub(crate) fn deserialize_dimension_type(&mut self) -> Result<DimensionType, ReadError> {
        let dim_num = self.read_u8()?;
        let dim =
            DimensionType::from_u8(dim_num).ok_or(ReadError::InvalidDimensionType(dim_num))?;
        self.ctx.initialize_section_count(dim);
        Ok(dim)
    }

    pub(crate) fn deserialize_palette_table(&mut self, table: &mut Vec<Palette>) -> Result<(), ReadError> {
        let len = self.read_u32()?;
        if len > MAX_PALETTE_TABLE_LENGTH {
            return Err(ReadError::LengthExceeded(
                "Palette table length too high".to_string(),
            ));
        }
        table.reserve(len as usize);

        for _ in 0..len {
            let palette_len = self.read_u16()? as usize;
            if palette_len > MAX_INDIRECT_PALETTE_SIZE {
                let skip_bytes = palette_len * std::mem::size_of::<SegmentAtom>();
                self.pos += skip_bytes;
                table.push(DIRECT_PALETTE.clone());
                continue;
            }

            let mut palette_vec = vec![0u16; palette_len];
            for atom in palette_vec.iter_mut() {
                *atom = self.read_u16()?;
            }
            let bpe = bits_per_entry(palette_len);
            table.push(Palette {
                palette: palette_vec.into(),
                bits_per_entry: bpe,
            });
        }
        Ok(())
    }

    pub(crate) fn deserialize_column_headers<const UNPACKED_SIZE: usize>(
        &mut self,
        column_length: usize,
        palette_table: &[Palette],
    ) -> Result<(
        Vec<PackedDeltaData<UNPACKED_SIZE>>,
        Vec<(usize, usize, usize, u8, bool)>,
    ), ReadError> {
        let mut counts = Vec::with_capacity(column_length);
        for _ in 0..column_length {
            let delta_length = self.read_u64()?;
            if delta_length > MAX_DELTA_LENGTH {
                return Err(ReadError::LengthExceeded(
                    "Delta length too high".to_string(),
                ));
            }
            counts.push(delta_length as usize);
        }

        let default_snapshot = PackedSnapshot {
            data: PackedData {
                data: Data::Single(0),
            },
            timestamp: 0,
        };

        let mut sections: Vec<PackedDeltaData<UNPACKED_SIZE>> = counts
            .iter()
            .map(|&count| PackedDeltaData {
                reverse_deltas: vec![default_snapshot.clone(); count],
            })
            .collect();

        let mut body_plan: Vec<(usize, usize, usize, u8, bool)> = Vec::new();

        for (section_index, &count) in counts.iter().enumerate() {
            for delta_index in 0..count {
                let active = self.max_deltas == 0 || delta_index < self.max_deltas;
                let timestamp = self.read_u64()? as i64;
                let data_type = self.read_u8()?;

                if data_type == 0 {
                    let single_val = self.read_u16()?;
                    if active {
                        sections[section_index].reverse_deltas[delta_index] = PackedSnapshot {
                            data: PackedData {
                                data: Data::Single(single_val),
                            },
                            timestamp,
                        };
                    }
                    continue;
                }

                let palette_index = self.read_u32()?;
                let bits_per_entry = if palette_index == u32::MAX {
                    16
                } else {
                    if palette_index as usize >= palette_table.len() {
                        return Err(ReadError::InvalidPaletteIndex {
                            index: palette_index,
                            max: palette_table.len(),
                        });
                    }
                    palette_table[palette_index as usize].bits_per_entry
                };
                let packed_len = UNPACKED_SIZE.div_ceil(64 / bits_per_entry);
                let direct = palette_index == u32::MAX;
                let plane_bytes = UNPACKED_SIZE / 8;
                let (byte_len, mask) = if direct {
                    (packed_len * std::mem::size_of::<u64>(), 0u8)
                } else {
                    let mask = self.read_u8()?;
                    (mask.count_ones() as usize * plane_bytes, mask)
                };

                if active {
                    let palette = if palette_index == u32::MAX {
                        DIRECT_PALETTE.clone()
                    } else {
                        palette_table[palette_index as usize].clone()
                    };
                    sections[section_index].reverse_deltas[delta_index] = PackedSnapshot {
                        data: PackedData {
                            data: Data::Paletted(PalettedData {
                                packed_long_array: Vec::with_capacity(packed_len),
                                palette,
                            }),
                        },
                        timestamp,
                    };
                    body_plan.push((section_index, delta_index, byte_len, mask, true));
                } else {
                    body_plan.push((section_index, delta_index, byte_len, mask, false));
                }
            }
        }

        Ok((sections, body_plan))
    }

    pub(crate) fn read_snapshot_body_into<const UNPACKED_SIZE: usize>(
        &mut self,
        byte_len: usize,
        mask: u8,
        active: bool,
        snapshot: &mut PackedSnapshot<UNPACKED_SIZE>,
    ) -> Result<(), ReadError> {
        if !active {
            self.pos += byte_len;
            return Ok(());
        }

        let Data::Paletted(paletted) = &mut snapshot.data.data else {
            return Ok(());
        };

        let bits = paletted.palette.bits_per_entry;
        let packed_len = UNPACKED_SIZE.div_ceil(64 / bits);

        if paletted.palette.direct() {
            let mut packed_longs: Vec<u64> = vec![0u64; packed_len];
            {
                let byte_slice = unsafe {
                    std::slice::from_raw_parts_mut(
                        packed_longs.as_mut_ptr() as *mut u8,
                        byte_len,
                    )
                };
                self.read_exact(byte_slice)?;
            }
            #[cfg(not(target_endian = "little"))]
            {
                for v in packed_longs.iter_mut() {
                    *v = v.to_le();
                }
            }
            paletted.packed_long_array = packed_longs;
            return Ok(());
        }

        if self.plane_scratch.len() < byte_len {
            self.plane_scratch.resize(byte_len, 0);
        }
        let scratch = &mut self.plane_scratch[..byte_len];
        self.cursor.read_exact(scratch)?;
        let mut packed_longs = vec![0u64; packed_len];
        super::bitplane::unpack_bitplanes_into::<UNPACKED_SIZE>(
            scratch,
            bits as u8,
            mask,
            &mut packed_longs,
        );
        super::bitplane::remap_from_popcount(&mut packed_longs, bits as u8);
        paletted.packed_long_array = packed_longs;
        Ok(())
    }

    pub(crate) fn deserialize_segment_state(&mut self) -> Result<SegmentState, ReadError> {
        let state_type_id = self.read_u8()?;
        let state_type = SegmentStateType::from_u8(state_type_id).ok_or_else(|| {
            ReadError::Generic(format!("Invalid segment state ID: {state_type_id}"))
        })?;
        let timestamp = self.read_u64()? as i64;
        Ok(SegmentState {
            state_type,
            timestamp,
        })
    }

    pub(crate) fn deserialize_segment_info(&mut self) -> Result<SegmentInfo, ReadError> {
        let states_len = self.read_u64()?;
        if states_len > MAX_SEGMENT_STATES_LENGTH {
            return Err(ReadError::LengthExceeded(
                "Segment states too long".to_string(),
            ));
        }

        let mut states = Vec::with_capacity(states_len as usize);
        for _ in 0..states_len {
            states.push(self.deserialize_segment_state()?);
        }
        Ok(SegmentInfo {
            reverse_deltas: states,
        })
    }

    pub(crate) fn deserialize_tile_entities(&mut self) -> Result<DeltaTileEntityData, ReadError> {
        let deltas_len = self.read_u64()?;
        if deltas_len > MAX_DELTA_LENGTH {
            return Err(ReadError::LengthExceeded(
                "Tile entity deltas length exceeded".to_string(),
            ));
        }

        let mut tile_entities = DeltaTileEntityData::default();
        tile_entities.reverse_deltas.reserve(deltas_len as usize);

        for _ in 0..deltas_len {
            let timestamp = self.read_u64()? as i64;
            let list_len = self.read_u64()?;
            if list_len > MAX_TILE_ENTITY_LIST_LENGTH {
                return Err(ReadError::LengthExceeded(
                    "Tile entity list length exceeded".to_string(),
                ));
            }

            let mut deltas_map = TileEntityDeltaMap::with_capacity(list_len as usize);
            for _ in 0..list_len {
                let pos = TileEntityPosition::unpack(self.read_u32()?);
                let op = self.read_u8()?;
                if op == 0 {
                    deltas_map.insert(pos, TileEntityDelta::Erase);
                    continue;
                }

                let tile_type = self.read_u32()?;
                let nbt_len = self.read_u64()?;
                if nbt_len > MAX_TILE_ENTITY_NBT_LENGTH {
                    return Err(ReadError::LengthExceeded(
                        "Tile entity NBT length exceeded".to_string(),
                    ));
                }

                let mut nbt = vec![0u8; nbt_len as usize];
                self.read_exact(&mut nbt)?;

                deltas_map.insert(
                    pos,
                    TileEntityDelta::Put(TileEntity {
                        tile_type,
                        pos,
                        nbt,
                    }),
                );
            }

            tile_entities.reverse_deltas.push(TileEntityListDelta {
                timestamp,
                deltas: deltas_map,
            });
        }
        Ok(tile_entities)
    }

    pub(crate) fn deserialize_region(&mut self, region: &mut Region) -> Result<(), ReadError> {
        let mut block_table = Vec::new();
        let mut biome_table = Vec::new();
        self.deserialize_palette_table(&mut block_table)?;
        self.deserialize_palette_table(&mut biome_table)?;

        let mut present = vec![false; SEGMENTS_PER_REGION];
        for v in present.iter_mut() {
            *v = self.read_u8()? != 0;
        }

        let present_indices: Vec<usize> = (0..SEGMENTS_PER_REGION).filter(|&i| present[i]).collect();
        let present_count = present_indices.len();
        let section_count = self.ctx.section_count;

        let mut segments: Vec<Option<Segment>> = (0..SEGMENTS_PER_REGION).map(|_| None).collect();
        for &i in &present_indices {
            segments[i] = Some(Segment::with_section_count(section_count));
        }

        let mut block_columns: Vec<Vec<PackedDeltaData<SECTION_SIZE_BLOCKS>>> =
            Vec::with_capacity(section_count);
        let mut block_body_plans: Vec<Vec<(usize, usize, usize, u8, bool)>> =
            Vec::with_capacity(section_count);
        for _ in 0..section_count {
            let (column, plan) =
                self.deserialize_column_headers::<SECTION_SIZE_BLOCKS>(present_count, &block_table)?;
            block_columns.push(column);
            block_body_plans.push(plan);
        }

        let mut biome_columns: Vec<Vec<PackedDeltaData<SECTION_SIZE_BIOMES>>> =
            Vec::with_capacity(section_count);
        let mut biome_body_plans: Vec<Vec<(usize, usize, usize, u8, bool)>> =
            Vec::with_capacity(section_count);
        for _ in 0..section_count {
            let (column, plan) =
                self.deserialize_column_headers::<SECTION_SIZE_BIOMES>(present_count, &biome_table)?;
            biome_columns.push(column);
            biome_body_plans.push(plan);
        }

        for y in 0..section_count {
            for k in 0..present_count {
                let i = present_indices[k];
                let seg = segments[i].as_mut().unwrap();
                seg.block_sections.sections[y] = std::mem::take(&mut block_columns[y][k]);
            }
        }
        for y in 0..section_count {
            for k in 0..present_count {
                let i = present_indices[k];
                let seg = segments[i].as_mut().unwrap();
                seg.biome_sections.sections[y] = std::mem::take(&mut biome_columns[y][k]);
            }
        }

        for y in 0..section_count {
            for &(k, j, byte_len, mask, active) in &block_body_plans[y] {
                let i = present_indices[k];
                let seg = segments[i].as_mut().unwrap();
                let snapshot = &mut seg.block_sections.sections[y].reverse_deltas[j];
                self.read_snapshot_body_into(byte_len, mask, active, snapshot)?;
            }
        }
        for y in 0..section_count {
            for &(k, j, byte_len, mask, active) in &biome_body_plans[y] {
                let i = present_indices[k];
                let seg = segments[i].as_mut().unwrap();
                let snapshot = &mut seg.biome_sections.sections[y].reverse_deltas[j];
                self.read_snapshot_body_into(byte_len, mask, active, snapshot)?;
            }
        }

        for &i in &present_indices {
            let seg = segments[i].as_mut().unwrap();
            seg.info = self.deserialize_segment_info()?;
        }
        for &i in &present_indices {
            let seg = segments[i].as_mut().unwrap();
            seg.tile_entities = self.deserialize_tile_entities()?;
        }

        for i in 0..SEGMENTS_PER_REGION {
            region.segments[i] = segments[i].take().map(Arc::new);
        }
        Ok(())
    }

    pub(crate) fn deserialize_file(&mut self) -> Result<File, ReadError> {
        self.validate_file_prefix(EXTENSION)?;
        let version = self.deserialize_version(ZVCR3D_LATEST_VERSION)?;
        let dimension_type = self.deserialize_dimension_type()?;
        let protocol_version = self.read_u16()?;

        self.ctx.protocol_version = protocol_version;

        let compressed_slice = &self.data[self.pos..];
        let uncompressed = decompress_zstd(compressed_slice).map_err(ReadError::Zstd)?;

        let mut region_handle = ReadHandle::new(uncompressed, self.max_deltas);
        region_handle.ctx = self.ctx.clone();

        let mut file = File {
            version,
            protocol_version,
            dimension_type,
            region: Region::new(protocol_version),
        };

        region_handle.deserialize_region(&mut file.region)?;
        Ok(file)
    }
}

#[allow(dead_code)]
pub(crate) fn read_file(filepath: &Path, max_deltas: usize) -> Result<File, ReadError> {
    let buffer = fs::read(filepath).map_err(|e| ReadError::FileNotFound(e.to_string()))?;
    let mut handle = ReadHandle::new(buffer, max_deltas);
    handle.deserialize_file()
}

#[allow(dead_code)]
pub(crate) fn read_file_at(
    parent_directory: &Path,
    location: &RegionLocation,
    max_deltas: usize,
) -> Result<File, ReadError> {
    read_file(&location.file_path(parent_directory), max_deltas)
}
