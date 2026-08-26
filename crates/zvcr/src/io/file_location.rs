use crate::dimension::DimensionType;
use std::path::{Path, PathBuf};

pub type RegionID = u64;
pub const SECTOR_SIDELENGTH: i32 = 32;
pub const ZVCR_REGION_PREFIX: &str = "r.";
pub const EXTENSION: &str = "zvcr3d";

pub fn floor_div_sector(rc: i32) -> i32 {
    rc.div_euclid(SECTOR_SIDELENGTH)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RegionLocation {
    pub rx: i32,
    pub rz: i32,
    pub dimension_type: DimensionType,
}

impl RegionLocation {
    pub fn to_region_id(&self) -> RegionID {
        let ux = (self.rx as u32 & 0x1F_FFFF) as u64;
        let uz = (self.rz as u32 & 0x1F_FFFF) as u64;
        let ud = self.dimension_type as u64;
        (ud << 42) | (ux << 21) | uz
    }

    pub fn from_region_id(region_id: RegionID) -> Self {
        let ux = ((region_id >> 21) & 0x1F_FFFF) as i32;
        let uz = (region_id & 0x1F_FFFF) as i32;
        let ud = ((region_id >> 42) & 0xFF_FFF) as u8;

        let recovered_x = if ux >= 0x10_0000 { ux - 0x20_0000 } else { ux };
        let recovered_z = if uz >= 0x10_0000 { uz - 0x20_0000 } else { uz };
        let recovered_dim = DimensionType::from_u8(ud).unwrap_or_default();

        Self {
            rx: recovered_x,
            rz: recovered_z,
            dimension_type: recovered_dim,
        }
    }

    pub fn from_file_name(dimension: DimensionType, file_path: &Path) -> Option<Self> {
        let filename = file_path.file_name()?.to_str()?;
        let dotted_extension = format!(".{EXTENSION}");
        if !filename.starts_with(ZVCR_REGION_PREFIX) || !filename.ends_with(&dotted_extension) {
            return None;
        }

        let start = ZVCR_REGION_PREFIX.len();
        let end = filename.len() - (EXTENSION.len() + 1);
        let identifier = &filename[start..end];
        let mut parts = identifier.split('.');

        let rx = parts.next()?.parse::<i32>().ok()?;
        let rz = parts.next()?.parse::<i32>().ok()?;

        Some(Self {
            rx,
            rz,
            dimension_type: dimension,
        })
    }

    pub fn directory(&self, parent_directory: &Path) -> PathBuf {
        let sector_x = floor_div_sector(self.rx).to_string();
        let sector_z = floor_div_sector(self.rz).to_string();
        let dim_id = self.dimension_type.name();

        parent_directory.join(dim_id).join(sector_x).join(sector_z)
    }

    pub fn file_name_extensionless(&self) -> String {
        format!("{}{}.{}", ZVCR_REGION_PREFIX, self.rx, self.rz)
    }

    pub fn file_name(&self) -> String {
        format!("{}.{}", self.file_name_extensionless(), EXTENSION)
    }

    pub fn file_path(&self, parent_directory: &Path) -> PathBuf {
        self.directory(parent_directory).join(self.file_name())
    }
}
