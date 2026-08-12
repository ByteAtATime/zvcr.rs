use super::File;
use crate::definitions::*;
use crate::dimension::DimensionType;
use crate::io::buffer::PooledBytes;
use crate::io::compression::decompress_zstd;
use crate::io::file_location::{EXTENSION, RegionLocation};
use crate::io::serialize::context::Context;
use crate::io::serialize::error::*;
use crate::io::serialize::primitives::ByteCursor;
use crate::raw::{RegionData, SegmentData};
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PackedData, PackedSnapshot, PalettedData};
use crate::region::palette::{DIRECT_PALETTE, MAX_INDIRECT_PALETTE_SIZE, Palette, bits_per_entry};
use crate::region::segment::*;
use crate::region::segment_info::*;
use crate::region::tile_entities::*;
use crate::version::*;
use std::fs;
use std::ops::{Deref, DerefMut, Range};
use std::path::Path;
use std::sync::Arc;

pub(crate) struct ReadHandle {
    pub(crate) ctx: Context,
    cursor: ByteCursor,
    max_deltas: usize,
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
    pub(crate) fn new(data: PooledBytes, max_deltas: usize) -> Self {
        Self {
            ctx: Context::default(),
            cursor: ByteCursor::new(data),
            max_deltas,
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

    pub(crate) fn deserialize_palette_table(
        &mut self,
        table: &mut Vec<Palette>,
    ) -> Result<(), ReadError> {
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

    pub(crate) fn deserialize_packed_snapshot_value<const UNPACKED_SIZE: usize>(
        &mut self,
        palette_table: &[Palette],
    ) -> Result<PackedSnapshot<UNPACKED_SIZE>, ReadError> {
        let timestamp = self.read_u64()? as i64;
        let data_type = self.read_u8()?;

        if data_type == 0 {
            let single_val = self.read_u16()?;
            return Ok(PackedSnapshot {
                data: PackedData {
                    data: Data::Single(single_val),
                },
                timestamp,
            });
        }

        let packed_length = self.read_u64()?;
        if packed_length > MAX_PACKED_LENGTH {
            return Err(ReadError::LengthExceeded(
                "Packed length invalid".to_string(),
            ));
        }

        let packed_bytes = self.take_slice(packed_length as usize * 8)?;

        let palette_index = self.read_u32()?;
        let palette = if palette_index == u32::MAX {
            DIRECT_PALETTE.clone()
        } else {
            if palette_index as usize >= palette_table.len() {
                return Err(ReadError::InvalidPaletteIndex {
                    index: palette_index,
                    max: palette_table.len(),
                });
            }
            palette_table[palette_index as usize].clone()
        };

        Ok(PackedSnapshot {
            data: PackedData {
                data: Data::Paletted(PalettedData {
                    packed_long_array: packed_bytes,
                    palette,
                }),
            },
            timestamp,
        })
    }

    pub(crate) fn skip_packed_snapshot(&mut self) -> Result<(), ReadError> {
        let _timestamp = self.read_u64()?;
        let data_type = self.read_u8()?;
        if data_type == 0 {
            let _single_val = self.read_u16()?;
            return Ok(());
        }

        let packed_length = self.read_u64()?;
        if packed_length > MAX_PACKED_LENGTH {
            return Err(ReadError::LengthExceeded(
                "Packed length invalid".to_string(),
            ));
        }
        self.skip(packed_length as usize * 8)?;
        let _palette_index = self.read_u32()?;
        Ok(())
    }

    pub(crate) fn read_section_group<const N: usize>(
        &mut self,
        section_count: usize,
        palette_table: &[Palette],
    ) -> Result<(Arc<Vec<PackedSnapshot<N>>>, Vec<Range<usize>>), ReadError> {
        let mut shared: Vec<PackedSnapshot<N>> = Vec::new();
        let mut ranges: Vec<Range<usize>> = Vec::with_capacity(section_count);

        for _ in 0..section_count {
            let start = shared.len();
            let delta_length_raw = self.read_u64()?;
            if delta_length_raw > MAX_DELTA_LENGTH {
                return Err(ReadError::LengthExceeded(
                    "Delta length too high".to_string(),
                ));
            }
            let delta_length = delta_length_raw as usize;
            shared.reserve(delta_length);
            for delta_index in 0..delta_length {
                if self.max_deltas != 0 && delta_index >= self.max_deltas {
                    self.skip_packed_snapshot()?;
                    continue;
                }
                shared.push(self.deserialize_packed_snapshot_value::<N>(palette_table)?);
            }
            ranges.push(start..shared.len());
        }

        Ok((Arc::new(shared), ranges))
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

    pub(crate) fn deserialize_segment(
        &mut self,
        block_tables: &[Palette],
        biome_tables: &[Palette],
    ) -> Result<Arc<Segment>, ReadError> {
        let sc = self.ctx.section_count;
        let mut segment = Segment::with_section_count(sc);

        let (block_storage, block_ranges) =
            self.read_section_group::<SECTION_SIZE_BLOCKS>(sc, block_tables)?;
        for (slot, r) in segment.block_sections.sections[..sc]
            .iter_mut()
            .zip(block_ranges)
        {
            *slot = PackedDeltaData::from_shared(Arc::clone(&block_storage), r);
        }
        let (biome_storage, biome_ranges) =
            self.read_section_group::<SECTION_SIZE_BIOMES>(sc, biome_tables)?;
        for (slot, r) in segment.biome_sections.sections[..sc]
            .iter_mut()
            .zip(biome_ranges)
        {
            *slot = PackedDeltaData::from_shared(Arc::clone(&biome_storage), r);
        }

        segment.info = self.deserialize_segment_info()?;
        segment.tile_entities = self.deserialize_tile_entities()?;
        Ok(Arc::new(segment))
    }

    pub(crate) fn deserialize_region(&mut self, region: &mut Region) -> Result<(), ReadError> {
        let mut block_table = Vec::new();
        let mut biome_table = Vec::new();

        self.deserialize_palette_table(&mut block_table)?;
        self.deserialize_palette_table(&mut biome_table)?;

        for segment in region.segments.iter_mut() {
            let indicator = self.read_u8()?;
            if indicator != 0 {
                *segment = Some(self.deserialize_segment(&block_table, &biome_table)?);
            }
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

        let mut region_handle =
            ReadHandle::new(PooledBytes::from_vec(uncompressed), self.max_deltas);
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

    #[allow(dead_code)]
    pub(crate) fn deserialize_to_region_data(&mut self) -> Result<RegionData, ReadError> {
        self.validate_file_prefix(EXTENSION)?;
        let version = self.deserialize_version(ZVCR3D_LATEST_VERSION)?;
        let dimension_type = self.deserialize_dimension_type()?;
        let protocol_version = self.read_u16()?;

        self.ctx.protocol_version = protocol_version;

        let compressed_slice = &self.data[self.pos..];
        let uncompressed = decompress_zstd(compressed_slice).map_err(ReadError::Zstd)?;

        let mut region_handle =
            ReadHandle::new(PooledBytes::from_vec(uncompressed), self.max_deltas);
        region_handle.ctx = self.ctx.clone();

        let mut rd = RegionData {
            version,
            protocol_version,
            dimension: dimension_type,
            segments: std::array::from_fn(|_| None),
        };

        region_handle.deserialize_region_data(&mut rd)?;
        Ok(rd)
    }

    pub(crate) fn deserialize_region_data(&mut self, rd: &mut RegionData) -> Result<(), ReadError> {
        let mut block_table = Vec::new();
        let mut biome_table = Vec::new();

        self.deserialize_palette_table(&mut block_table)?;
        self.deserialize_palette_table(&mut biome_table)?;

        for slot in rd.segments.iter_mut() {
            let indicator = self.read_u8()?;
            if indicator != 0 {
                *slot = Some(self.deserialize_segment_data(&block_table, &biome_table)?);
            }
        }
        Ok(())
    }

    pub(crate) fn deserialize_segment_data(
        &mut self,
        block_tables: &[Palette],
        biome_tables: &[Palette],
    ) -> Result<SegmentData, ReadError> {
        let sc = self.ctx.section_count;

        let (block_storage, block_ranges) =
            self.read_section_group::<SECTION_SIZE_BLOCKS>(sc, block_tables)?;
        let block_sections: Vec<PackedDeltaData<SECTION_SIZE_BLOCKS>> = block_ranges
            .into_iter()
            .map(|r| PackedDeltaData::from_shared(Arc::clone(&block_storage), r))
            .collect();

        let (biome_storage, biome_ranges) =
            self.read_section_group::<SECTION_SIZE_BIOMES>(sc, biome_tables)?;
        let biome_sections: Vec<PackedDeltaData<SECTION_SIZE_BIOMES>> = biome_ranges
            .into_iter()
            .map(|r| PackedDeltaData::from_shared(Arc::clone(&biome_storage), r))
            .collect();

        let info = self.deserialize_segment_info()?;
        let tile_entities = self.deserialize_tile_entities()?;

        Ok(SegmentData {
            block_sections,
            biome_sections,
            states: info.reverse_deltas,
            tile_entities,
        })
    }
}

#[allow(dead_code)]
pub(crate) fn read_file(filepath: &Path, max_deltas: usize) -> Result<File, ReadError> {
    let buffer = fs::read(filepath).map_err(|e| ReadError::FileNotFound(e.to_string()))?;
    let mut handle = ReadHandle::new(PooledBytes::from_vec(buffer), max_deltas);
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
