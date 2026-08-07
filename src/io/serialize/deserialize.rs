use crate::File;
use crate::definitions::*;
use crate::dimension::DimensionType;
use crate::io::compression::decompress_zstd;
use crate::io::file_location::{RegionLocation, EXTENSION};
use crate::io::serialize::context::Context;
use crate::region::paletted_delta_data::*;
use crate::region::segment::*;
use crate::region::segment_info::*;
use crate::region::tile_entities::*;
use crate::version::*;
use std::fs;
use std::path::Path;
use std::sync::Arc;

pub const MAX_DELTA_LENGTH: u64 = 65536;
pub const MAX_SEGMENT_STATES_LENGTH: u64 = 65536;
pub const MAX_TILE_ENTITY_LIST_LENGTH: u64 = 98304;
pub const MAX_TILE_ENTITY_NBT_LENGTH: u64 = 65536;
pub const MAX_PACKED_LENGTH: u64 = 1024;
pub const MAX_PALETTE_TABLE_LENGTH: u32 = 262144;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("Generic read error: {0}")]
    Generic(String),
    #[error("ZSTD error: {0}")]
    Zstd(String),
    #[error("Read out of bounds at offset {offset}")]
    OutOfBounds { offset: usize },
    #[error("Invalid version: {0}")]
    InvalidVersion(u8),
    #[error("Invalid dimension type: {0}")]
    InvalidDimensionType(u8),
    #[error("Invalid palette index: {index} >= {max}")]
    InvalidPaletteIndex { index: u32, max: usize },
    #[error("Header prefix mismatch")]
    HeaderMismatch,
    #[error("Length constraint exceeded: {0}")]
    LengthExceeded(String),
}

pub struct ReadHandle {
    pub ctx: Context,
    data: Vec<u8>,
    pos: usize,
    max_deltas: usize,
    block_palette_table: Vec<Palette>,
    biome_palette_table: Vec<Palette>,
}

impl ReadHandle {
    pub fn new(data: Vec<u8>, max_deltas: usize) -> Self {
        Self {
            ctx: Context::default(),
            data,
            pos: 0,
            max_deltas,
            block_palette_table: Vec::new(),
            biome_palette_table: Vec::new(),
        }
    }

    pub fn offset(&self) -> usize {
        self.pos
    }

    fn read_bytes<const N: usize>(&mut self) -> Result<[u8; N], ReadError> {
        if self.pos + N > self.data.len() {
            return Err(ReadError::OutOfBounds { offset: self.pos });
        }
        let mut bytes = [0u8; N];
        bytes.copy_from_slice(&self.data[self.pos..self.pos + N]);
        self.pos += N;
        Ok(bytes)
    }

    fn read_u8(&mut self) -> Result<u8, ReadError> {
        Ok(self.read_bytes::<1>()?[0])
    }

    fn read_u16(&mut self) -> Result<u16, ReadError> {
        Ok(u16::from_le_bytes(self.read_bytes::<2>()?))
    }

    fn read_u32(&mut self) -> Result<u32, ReadError> {
        Ok(u32::from_le_bytes(self.read_bytes::<4>()?))
    }

    fn read_u64(&mut self) -> Result<u64, ReadError> {
        Ok(u64::from_le_bytes(self.read_bytes::<8>()?))
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ReadError> {
        let n = buf.len();
        if self.pos + n > self.data.len() {
            return Err(ReadError::OutOfBounds { offset: self.pos });
        }
        buf.copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(())
    }

    pub fn validate_file_prefix(&mut self, prefix: &str) -> Result<(), ReadError> {
        let mut buf = vec![0u8; prefix.len()];
        self.read_exact(&mut buf)?;
        if buf != prefix.as_bytes() {
            return Err(ReadError::HeaderMismatch);
        }
        Ok(())
    }

    pub fn deserialize_version(&mut self, latest: Version) -> Result<Version, ReadError> {
        let ver_num = self.read_u8()?;
        if ver_num > latest as u8 {
            return Err(ReadError::InvalidVersion(ver_num));
        }
        Version::from_u8(ver_num).ok_or(ReadError::InvalidVersion(ver_num))
    }

    pub fn deserialize_dimension_type(&mut self) -> Result<DimensionType, ReadError> {
        let dim_num = self.read_u8()?;
        let dim =
            DimensionType::from_u8(dim_num).ok_or(ReadError::InvalidDimensionType(dim_num))?;
        self.ctx.initialize_section_count(dim);
        Ok(dim)
    }

    pub fn deserialize_palette_table(&mut self, table: &mut Vec<Palette>) -> Result<(), ReadError> {
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
                table.push(DIRECT_PALETTE);
                continue;
            }

            let mut palette_vec = vec![0u16; palette_len];
            for i in 0..palette_len {
                palette_vec[i] = self.read_u16()?;
            }
            let bpe = bits_per_entry(palette_len);
            table.push(Palette {
                palette: palette_vec,
                bits_per_entry: bpe,
            });
        }
        Ok(())
    }

    pub fn deserialize_packed_snapshot<const UNPACKED_SIZE: usize>(
        &mut self,
        snapshot: &mut PackedSnapshot<UNPACKED_SIZE>,
        palette_table: &[Palette],
    ) -> Result<(), ReadError> {
        snapshot.timestamp = self.read_u64()? as i64;
        let data_type = self.read_u8()?;

        if data_type == 0 {
            let single_val = self.read_u16()?;
            snapshot.data = PackedData {
                data: Data::Single(single_val),
            };
            return Ok(());
        }

        let packed_length = self.read_u64()?;
        if packed_length > MAX_PACKED_LENGTH {
            return Err(ReadError::LengthExceeded(
                "Packed length invalid".to_string(),
            ));
        }

        let mut packed_longs = vec![0u64; packed_length as usize];
        for i in 0..packed_length as usize {
            packed_longs[i] = self.read_u64()?;
        }

        let palette_index = self.read_u32()?;
        let palette = if palette_index == u32::MAX {
            DIRECT_PALETTE
        } else {
            if palette_index as usize >= palette_table.len() {
                return Err(ReadError::InvalidPaletteIndex {
                    index: palette_index,
                    max: palette_table.len(),
                });
            }
            palette_table[palette_index as usize].clone()
        };

        snapshot.data = PackedData {
            data: Data::Paletted(PalettedData {
                packed_long_array: packed_longs,
                palette,
            }),
        };
        Ok(())
    }

    pub fn deserialize_packed_delta_data<const UNPACKED_SIZE: usize>(
        &mut self,
        deltas: &mut PackedDeltaData<UNPACKED_SIZE>,
        palette_table: &[Palette],
    ) -> Result<(), ReadError> {
        let delta_length = self.read_u64()?;
        if delta_length > MAX_DELTA_LENGTH {
            return Err(ReadError::LengthExceeded(
                "Delta length too high".to_string(),
            ));
        }

        deltas.reverse_deltas.resize(
            delta_length as usize,
            PackedSnapshot {
                data: PackedData {
                    data: Data::Single(0),
                },
                timestamp: 0,
            },
        );

        for delta_index in 0..delta_length as usize {
            if self.max_deltas != 0 && delta_index >= self.max_deltas {
                // Skip packed snapshot logic
                let _ts = self.read_u64()?;
                let dtype = self.read_u8()?;
                if dtype == 0 {
                    let _s = self.read_u16()?;
                } else {
                    let plen = self.read_u64()?;
                    self.pos += (plen * 8) as usize;
                    let _pindex = self.read_u32()?;
                }
                continue;
            }
            self.deserialize_packed_snapshot(
                &mut deltas.reverse_deltas[delta_index],
                palette_table,
            )?;
        }
        Ok(())
    }

    pub fn deserialize_segment_state(&mut self) -> Result<SegmentState, ReadError> {
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

    pub fn deserialize_segment_info(&mut self) -> Result<SegmentInfo, ReadError> {
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
            segment_states: states,
        })
    }

    pub fn deserialize_tile_entities(&mut self) -> Result<DeltaTileEntityData, ReadError> {
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

    pub fn deserialize_segment(&mut self) -> Result<Arc<Segment>, ReadError> {
        let mut segment = Segment::with_section_count(self.ctx.section_count);
        let block_tables = self.block_palette_table.clone();
        let biome_tables = self.biome_palette_table.clone();

        for i in 0..self.ctx.section_count {
            self.deserialize_packed_delta_data(
                &mut segment.block_sections.sections[i],
                &block_tables,
            )?;
        }
        for i in 0..self.ctx.section_count {
            self.deserialize_packed_delta_data(
                &mut segment.biome_sections.sections[i],
                &biome_tables,
            )?;
        }

        segment.info = self.deserialize_segment_info()?;
        segment.tile_entities = self.deserialize_tile_entities()?;
        Ok(Arc::new(segment))
    }

    pub fn deserialize_region(&mut self, region: &mut Region) -> Result<(), ReadError> {
        let mut block_table = Vec::new();
        let mut biome_table = Vec::new();

        self.deserialize_palette_table(&mut block_table)?;
        self.deserialize_palette_table(&mut biome_table)?;

        self.block_palette_table = block_table;
        self.biome_palette_table = biome_table;

        for i in 0..SEGMENTS_PER_REGION {
            let indicator = self.read_u8()?;
            if indicator != 0 {
                region.segments[i] = Some(self.deserialize_segment()?);
            }
        }
        Ok(())
    }

    pub fn deserialize_file(&mut self) -> Result<File, ReadError> {
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

pub fn read_file(filepath: &Path, max_deltas: usize) -> Result<File, ReadError> {
    let buffer = fs::read(filepath).map_err(|e| ReadError::FileNotFound(e.to_string()))?;
    let mut handle = ReadHandle::new(buffer, max_deltas);
    handle.deserialize_file()
}

pub fn read_file_at(
    parent_directory: &Path,
    location: &RegionLocation,
    max_deltas: usize,
) -> Result<File, ReadError> {
    read_file(&location.file_path(parent_directory), max_deltas)
}
