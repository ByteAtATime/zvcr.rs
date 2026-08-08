use crate::dimension::DimensionType;
use crate::region::segment::Region;
use crate::version::{Version, ZVCR3D_LATEST_VERSION};

pub(crate) const DEFAULT_PROTOCOL_VERSION: u16 = 769;

#[derive(Debug, Clone)]
pub(crate) struct File {
    pub(crate) version: Version,
    pub(crate) protocol_version: u16,
    pub(crate) dimension_type: DimensionType,
    pub(crate) region: Region,
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
