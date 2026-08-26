use crate::dimension::DimensionType;

pub const PROTOCOL_VERSION_ZVCR_0_0_0_X: u16 = 765;

#[derive(Debug, Clone, Default)]
pub struct Context {
    pub section_count: usize,
    pub protocol_version: u16,
}

impl Context {
    pub fn initialize_section_count(&mut self, dimension_type: DimensionType) {
        self.section_count = dimension_type.section_count();
    }
}
