use crate::io::serialize::error::{MAX_PALETTE_TABLE_LENGTH, ReadError};
use crate::io::serialize::primitives::{ByteCursor, put_u16_le, put_u32_le};
use crate::region::palette::{DIRECT_PALETTE, MAX_INDIRECT_PALETTE_SIZE, Palette, bits_per_entry};
use ahash::AHashMap;

pub(crate) struct PaletteTable {
    entries: AHashMap<Palette, usize>,
    finalized: bool,
}

impl PaletteTable {
    pub(crate) fn new() -> Self {
        Self {
            entries: AHashMap::new(),
            finalized: false,
        }
    }

    pub(crate) fn record(&mut self, palette: &Palette) {
        if palette.direct() {
            return;
        }
        if let Some(count) = self.entries.get_mut(palette) {
            *count += 1;
        } else {
            self.entries.insert(palette.clone(), 1);
        }
    }

    pub(crate) fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        let mut ordering: Vec<(Palette, usize)> = self.entries.drain().collect();
        ordering.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.palette.cmp(&b.0.palette)));
        self.entries = AHashMap::with_capacity(ordering.len());
        for (rank, (palette, _)) in ordering.into_iter().enumerate() {
            self.entries.insert(palette, rank);
        }
        self.finalized = true;
    }

    pub(crate) fn index_for(&self, palette: &Palette) -> u32 {
        if palette.direct() {
            return u32::MAX;
        }
        self.entries[palette] as u32
    }

    pub(crate) fn serialize(&self, buf: &mut Vec<u8>) {
        let mut ordered = vec![DIRECT_PALETTE.clone(); self.entries.len()];
        for (palette, &index) in &self.entries {
            ordered[index] = palette.clone();
        }

        put_u32_le(buf, ordered.len() as u32);
        for palette in &ordered {
            let len = palette.length();
            if palette.direct() || len == 1 {
                continue;
            }
            put_u16_le(buf, len as u16);
            for &atom in palette.palette.iter() {
                put_u16_le(buf, atom);
            }
        }
    }
}

pub(crate) fn deserialize_palette_table(
    cursor: &mut ByteCursor,
) -> Result<Vec<Palette>, ReadError> {
    let len = cursor.read_u32()?;
    if len > MAX_PALETTE_TABLE_LENGTH {
        return Err(ReadError::LengthExceeded(
            "Palette table length too high".to_string(),
        ));
    }

    let mut table = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let palette_len = cursor.read_u16()? as usize;
        if palette_len > MAX_INDIRECT_PALETTE_SIZE {
            cursor.skip(palette_len * std::mem::size_of::<u16>())?;
            table.push(DIRECT_PALETTE.clone());
            continue;
        }

        let mut palette_vec = Vec::with_capacity(palette_len);
        for _ in 0..palette_len {
            palette_vec.push(cursor.read_u16()?);
        }
        table.push(Palette {
            palette: palette_vec.into(),
            bits_per_entry: bits_per_entry(palette_len),
        });
    }
    Ok(table)
}

pub(crate) fn palette_at<'a>(
    palette_index: u32,
    table: &'a [Palette],
) -> Result<&'a Palette, ReadError> {
    if palette_index == u32::MAX {
        return Ok(&DIRECT_PALETTE);
    }
    let idx = palette_index as usize;
    if idx >= table.len() {
        return Err(ReadError::InvalidPaletteIndex {
            index: palette_index,
            max: table.len(),
        });
    }
    Ok(&table[idx])
}
