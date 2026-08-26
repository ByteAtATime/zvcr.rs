#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(u8)]
pub enum Version {
    Zvcr3d0001 = 1,
    Zvcr3d0100 = 2,
    Zvcr3d0110 = 3,
    Zvcr3d0120 = 4,
    Zvcr3d0130 = 5,
    Zvcr3d0140 = 6,
    Zvcr3d1000 = 7,
    #[default]
    Zvcr3d1001 = 8,
}

pub const ZVCR3D_LATEST_VERSION: Version = Version::Zvcr3d1001;

impl Version {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Zvcr3d0001),
            2 => Some(Self::Zvcr3d0100),
            3 => Some(Self::Zvcr3d0110),
            4 => Some(Self::Zvcr3d0120),
            5 => Some(Self::Zvcr3d0130),
            6 => Some(Self::Zvcr3d0140),
            7 => Some(Self::Zvcr3d1000),
            8 => Some(Self::Zvcr3d1001),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Zvcr3d0001 => "0.0.0.1",
            Self::Zvcr3d0100 => "0.1.0.0",
            Self::Zvcr3d0110 => "0.1.1.0",
            Self::Zvcr3d0120 => "0.1.2.0",
            Self::Zvcr3d0130 => "0.1.3.0",
            Self::Zvcr3d0140 => "0.1.4.0",
            Self::Zvcr3d1000 => "1.0.0.0",
            Self::Zvcr3d1001 => "1.0.0.1",
        }
    }
}
