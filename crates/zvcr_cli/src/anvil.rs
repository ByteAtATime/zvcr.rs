const SECTOR_SIZE: usize = 4096;
const REGION_SIDE: usize = 32;
const CHUNK_COUNT: usize = REGION_SIDE * REGION_SIDE;
const MAX_PAYLOAD_BYTES: usize = 1048571;

struct ChunkEntry {
    timestamp: u32,
    payload: Vec<u8>,
}

pub struct AnvilRegionWriter {
    chunks: [Option<ChunkEntry>; CHUNK_COUNT],
}

impl AnvilRegionWriter {
    pub fn new() -> Self {
        Self {
            chunks: std::array::from_fn(|_| None),
        }
    }

    pub fn write_chunk(
        &mut self,
        local_x: usize,
        local_z: usize,
        timestamp: u32,
        zlib_compressed_payload: &[u8],
    ) -> Result<(), String> {
        if local_x >= REGION_SIDE || local_z >= REGION_SIDE {
            return Err(format!(
                "chunk coordinates out of region bounds: {local_x}, {local_z}"
            ));
        }
        if zlib_compressed_payload.len() >= MAX_PAYLOAD_BYTES {
            return Err("oversized chunk unsupported".to_string());
        }
        let index = local_z * REGION_SIDE + local_x;
        self.chunks[index] = Some(ChunkEntry {
            timestamp,
            payload: zlib_compressed_payload.to_vec(),
        });
        Ok(())
    }

    pub fn finish(&self) -> Result<Vec<u8>, String> {
        let mut offsets = [0u8; SECTOR_SIZE];
        let mut timestamps = [0u8; SECTOR_SIZE];
        let mut sector_offset: u32 = 2;
        let mut chunk_writes: Vec<(u32, Vec<u8>)> = Vec::new();

        for index in 0..CHUNK_COUNT {
            let Some(entry) = &self.chunks[index] else {
                continue;
            };
            let payload_len = entry.payload.len() as u32;
            let on_disk_len = payload_len + 5;
            let sector_count = (on_disk_len as usize).div_ceil(SECTOR_SIZE);
            if sector_count >= 256 {
                return Err("oversized chunk unsupported".to_string());
            }
            let base = index * 4;
            offsets[base] = (sector_offset >> 16) as u8;
            offsets[base + 1] = (sector_offset >> 8) as u8;
            offsets[base + 2] = sector_offset as u8;
            offsets[base + 3] = sector_count as u8;
            timestamps[base] = (entry.timestamp >> 24) as u8;
            timestamps[base + 1] = (entry.timestamp >> 16) as u8;
            timestamps[base + 2] = (entry.timestamp >> 8) as u8;
            timestamps[base + 3] = entry.timestamp as u8;

            let mut data = Vec::with_capacity(on_disk_len as usize);
            data.extend_from_slice(&payload_len.to_be_bytes());
            data.push(0x02);
            data.extend_from_slice(&entry.payload);
            chunk_writes.push((sector_offset, data));
            sector_offset += sector_count as u32;
        }

        let total_bytes = (sector_offset as usize) * SECTOR_SIZE;
        let mut out = vec![0u8; total_bytes];
        out[..SECTOR_SIZE].copy_from_slice(&offsets);
        out[SECTOR_SIZE..2 * SECTOR_SIZE].copy_from_slice(&timestamps);
        for (offset, data) in chunk_writes {
            let start = (offset as usize) * SECTOR_SIZE;
            out[start..start + data.len()].copy_from_slice(&data);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::read::ZlibDecoder;
    use flate2::write::ZlibEncoder;
    use std::io::{Read, Write};

    fn compress(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn test_region_roundtrip() {
        let mut writer = AnvilRegionWriter::new();
        let original1 = b"chunk one payload contents here".to_vec();
        let original2 = b"second chunk with different contents".to_vec();
        let comp1 = compress(&original1);
        let comp2 = compress(&original2);
        writer.write_chunk(0, 0, 1000, &comp1).unwrap();
        writer.write_chunk(5, 3, 2000, &comp2).unwrap();
        let bytes = writer.finish().unwrap();

        let count0 = ((comp1.len() + 5 + SECTOR_SIZE - 1) / SECTOR_SIZE) as u32;
        let expected_len =
            ((2 + count0 + ((comp2.len() + 5 + SECTOR_SIZE - 1) / SECTOR_SIZE) as u32) as usize)
                * SECTOR_SIZE;
        assert_eq!(bytes.len(), expected_len);

        let o0 = 0usize * 4;
        let offset0 = (bytes[o0] as u32) << 16 | (bytes[o0 + 1] as u32) << 8 | bytes[o0 + 2] as u32;
        let count0_actual = bytes[o0 + 3];
        assert_eq!(offset0, 2);
        assert_eq!(count0_actual, count0 as u8);

        let start0 = offset0 as usize * SECTOR_SIZE;
        let plen0 = u32::from_be_bytes([
            bytes[start0],
            bytes[start0 + 1],
            bytes[start0 + 2],
            bytes[start0 + 3],
        ]);
        assert_eq!(plen0, comp1.len() as u32);
        assert_eq!(bytes[start0 + 4], 0x02);
        let stored0 = &bytes[start0 + 5..start0 + 5 + comp1.len()];
        assert_eq!(stored0, &comp1[..]);

        let mut decoder = ZlibDecoder::new(stored0);
        let mut decoded = Vec::new();
        decoder.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, original1);

        let o1 = 101usize * 4;
        let offset1 = (bytes[o1] as u32) << 16 | (bytes[o1 + 1] as u32) << 8 | bytes[o1 + 2] as u32;
        assert_eq!(offset1, 2 + count0 as u32);
        let ts1 = u32::from_be_bytes([
            bytes[SECTOR_SIZE + o1],
            bytes[SECTOR_SIZE + o1 + 1],
            bytes[SECTOR_SIZE + o1 + 2],
            bytes[SECTOR_SIZE + o1 + 3],
        ]);
        assert_eq!(ts1, 2000);

        let start1 = offset1 as usize * SECTOR_SIZE;
        let plen1 = u32::from_be_bytes([
            bytes[start1],
            bytes[start1 + 1],
            bytes[start1 + 2],
            bytes[start1 + 3],
        ]);
        assert_eq!(plen1, comp2.len() as u32);
        assert_eq!(bytes[start1 + 4], 0x02);
        let stored1 = &bytes[start1 + 5..start1 + 5 + comp2.len()];
        let mut decoder2 = ZlibDecoder::new(stored1);
        let mut decoded2 = Vec::new();
        decoder2.read_to_end(&mut decoded2).unwrap();
        assert_eq!(decoded2, original2);

        let oa = 1usize * 4;
        assert_eq!(bytes[oa], 0);
        assert_eq!(bytes[oa + 1], 0);
        assert_eq!(bytes[oa + 2], 0);
        assert_eq!(bytes[oa + 3], 0);
        let tsa = u32::from_be_bytes([
            bytes[SECTOR_SIZE + oa],
            bytes[SECTOR_SIZE + oa + 1],
            bytes[SECTOR_SIZE + oa + 2],
            bytes[SECTOR_SIZE + oa + 3],
        ]);
        assert_eq!(tsa, 0);
    }

    #[test]
    fn test_oversized_rejected() {
        let mut writer = AnvilRegionWriter::new();
        let big = vec![0u8; 1048571];
        let result = writer.write_chunk(0, 0, 0, &big);
        assert!(result.is_err());
    }

    #[test]
    fn test_out_of_bounds_rejected() {
        let mut writer = AnvilRegionWriter::new();
        let payload = vec![1u8, 2, 3];
        assert!(writer.write_chunk(32, 0, 0, &payload).is_err());
        assert!(writer.write_chunk(0, 32, 0, &payload).is_err());
    }
}
