use std::cell::RefCell;

use crate::io::serialize::experimental::layout::{self, Domain};
use crate::io::serialize::experimental::pack::{ReusablePackScratch, pack_reused};
use crate::io::serialize::primitives::{put_bytes, put_u16_le, put_u32_le, put_u64_le};
use crate::raw::{RegionData, SegmentData};
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::Data;
use crate::region::palette::ATOM_COUNT;
use crate::region::unpacked_view::UnpackedData;

use super::Streams;

thread_local! {
    static GLOBAL_LUT: RefCell<Box<[u32; ATOM_COUNT]>> =
        RefCell::new(Box::new([u32::MAX; ATOM_COUNT]));
    static PACK: RefCell<ReusablePackScratch> = RefCell::new(ReusablePackScratch::new());
}

pub(super) fn write_domain<const UNPACKED_SIZE: usize>(
    streams: &mut Streams,
    data: &RegionData,
    domain: Domain,
    sections: fn(&SegmentData) -> &[PackedDeltaData<UNPACKED_SIZE>],
) -> Result<(), String> {
    GLOBAL_LUT
        .with(|cell| write_domain_streams(streams, data, domain, sections, &mut *cell.borrow_mut()))
}

fn write_domain_streams<const UNPACKED_SIZE: usize>(
    streams: &mut Streams,
    data: &RegionData,
    domain: Domain,
    sections: fn(&SegmentData) -> &[PackedDeltaData<UNPACKED_SIZE>],
    lut: &mut [u32; ATOM_COUNT],
) -> Result<(), String> {
    lut.fill(u32::MAX);
    for segment in data.segments.iter().flatten() {
        for section in sections(segment) {
            for snapshot in section.snapshots() {
                if let Data::Paletted(_) = &snapshot.data.data {
                    for atom in snapshot.data.unpack() {
                        lut[atom as usize] = 1;
                    }
                }
            }
        }
    }

    let atoms = finalize_palette(lut);
    put_u32_le(&mut streams.global_palette, atoms.len() as u32);
    for &atom in &atoms {
        put_u16_le(&mut streams.global_palette, atom);
    }

    let levels = domain_levels(data, sections);
    for level in 0..levels {
        for segment in data.segments.iter().flatten() {
            for section in sections(segment) {
                let snapshots = section.snapshots();
                if snapshots.len() <= level {
                    continue;
                }
                let snapshot = &snapshots[level];
                put_u64_le(&mut streams.timestamps, snapshot.timestamp as u64);
                match &snapshot.data.data {
                    Data::Single(atom) => put_u16_le(&mut streams.singles, *atom),
                    Data::Paletted(_) => {
                        let grid = snapshot.data.unpack();
                        PACK.with(|pack| {
                            emit_paletted(streams, domain, &grid, lut, &mut *pack.borrow_mut())
                        })?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn domain_levels<const UNPACKED_SIZE: usize>(
    data: &RegionData,
    sections: fn(&SegmentData) -> &[PackedDeltaData<UNPACKED_SIZE>],
) -> usize {
    data.segments
        .iter()
        .flatten()
        .flat_map(|segment| sections(segment).iter())
        .map(|section| section.snapshots().len())
        .max()
        .unwrap_or(0)
}

fn finalize_palette(lut: &mut [u32; ATOM_COUNT]) -> Vec<u16> {
    let mut atoms = Vec::new();
    for atom in 0..ATOM_COUNT {
        if lut[atom] == 1 {
            atoms.push(atom as u16);
        }
    }
    for (index, &atom) in atoms.iter().enumerate() {
        lut[atom as usize] = index as u32;
    }
    atoms
}

fn emit_paletted<const UNPACKED_SIZE: usize>(
    streams: &mut Streams,
    domain: Domain,
    grid: &UnpackedData<UNPACKED_SIZE>,
    lut: &[u32; ATOM_COUNT],
    scratch: &mut ReusablePackScratch,
) -> Result<(), String> {
    match pack_reused(grid, scratch).data {
        Data::Single(atom) => {
            let index = lut[atom as usize];
            if index == u32::MAX {
                return Err(format!("global palette miss for atom {atom:#06x}"));
            }
            put_u16_le(&mut streams.local_palettes, 1);
            put_u16_le(&mut streams.local_palettes, index as u16);
            let bucket = domain.bucket(1);
            let zero_bytes = layout::packed_byte_len(UNPACKED_SIZE, 1);
            let packed = &mut streams.buckets[bucket];
            packed.resize(packed.len() + zero_bytes, 0);
            streams.bucket_counts[bucket] += 1;
        }
        Data::Paletted(paletted) => {
            let bucket = domain.bucket(paletted.palette.bits_per_entry);
            streams.bucket_counts[bucket] += 1;
            if paletted.palette.direct() {
                put_u16_le(&mut streams.local_palettes, 0);
                put_bytes(&mut streams.buckets[bucket], &paletted.packed_long_array);
            } else {
                put_u16_le(
                    &mut streams.local_palettes,
                    paletted.palette.palette.len() as u16,
                );
                for &atom in paletted.palette.palette.iter() {
                    let index = lut[atom as usize];
                    if index == u32::MAX {
                        return Err(format!("global palette miss for atom {atom:#06x}"));
                    }
                    put_u16_le(&mut streams.local_palettes, index as u16);
                }
                put_bytes(&mut streams.buckets[bucket], &paletted.packed_long_array);
            }
        }
    }
    Ok(())
}
