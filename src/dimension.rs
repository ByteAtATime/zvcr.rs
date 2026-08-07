use crate::definitions::SEGMENT_SIDELENGTH_BLOCKS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DimensionProperties {
    pub has_sky_light: bool,
    pub min_y: i32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum DimensionType {
    #[default]
    Overworld = 0,
    Nether = 1,
    TheEnd = 2,
}

impl DimensionType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Overworld),
            1 => Some(Self::Nether),
            2 => Some(Self::TheEnd),
            _ => None,
        }
    }

    pub fn properties(self) -> DimensionProperties {
        match self {
            Self::Overworld => DimensionProperties {
                has_sky_light: true,
                min_y: -64,
                height: 384,
            },
            Self::Nether => DimensionProperties {
                has_sky_light: false,
                min_y: 0,
                height: 256,
            },
            Self::TheEnd => DimensionProperties {
                has_sky_light: false,
                min_y: 0,
                height: 256,
            },
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Overworld => "overworld",
            Self::Nether => "nether",
            Self::TheEnd => "end",
        }
    }

    pub fn min_section_y(self) -> i32 {
        self.properties().min_y / SEGMENT_SIDELENGTH_BLOCKS as i32
    }

    pub fn section_count(self) -> usize {
        self.properties().height as usize / SEGMENT_SIDELENGTH_BLOCKS
    }
}
