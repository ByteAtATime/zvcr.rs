use std::cell::RefCell;

use crate::io::serialize::experimental::layout::{self, Domain};
use crate::io::serialize::experimental::pack::{
    MAX_CELLS, ReusablePackScratch, extract_atoms, extract_direct_atoms, extract_indices,
    pack_atoms_le, pack_values, remap_values,
};
use crate::io::serialize::primitives::{put_bytes, put_u16_le, put_u32_le, put_u64_le};
use crate::raw::{RegionData, SegmentData};
use crate::region::delta::PackedDeltaData;
use crate::region::packed_data::{Data, PalettedData};
use crate::region::palette::{ATOM_COUNT, MAX_INDIRECT_PALETTE_SIZE, bits_per_entry};

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
        .with(|cell| write_domain_streams(streams, data, domain, sections, &mut cell.borrow_mut()))
}

fn write_domain_streams<const UNPACKED_SIZE: usize>(
    streams: &mut Streams,
    data: &RegionData,
    domain: Domain,
    sections: fn(&SegmentData) -> &[PackedDeltaData<UNPACKED_SIZE>],
    lut: &mut [u32; ATOM_COUNT],
) -> Result<(), String> {
    lut.fill(u32::MAX);
    let modeled = domain == Domain::Block;
    for segment in data.segments.iter().flatten() {
        for section in sections(segment) {
            for (level, snapshot) in section.snapshots().iter().enumerate() {
                if modeled && level == 0 {
                    continue;
                }
                let Data::Paletted(paletted) = &snapshot.data.data else {
                    continue;
                };
                mark_global_atoms(paletted, lut);
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
                if modeled && level == 0 {
                    continue;
                }
                match &snapshot.data.data {
                    Data::Single(atom) => put_u16_le(&mut streams.singles, *atom),
                    Data::Paletted(paletted) => {
                        PACK.with(|pack| {
                            emit_paletted(streams, domain, paletted, lut, &mut pack.borrow_mut())
                        })?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn mark_global_atoms<const UNPACKED_SIZE: usize>(
    paletted: &PalettedData<UNPACKED_SIZE>,
    lut: &mut [u32; ATOM_COUNT],
) {
    if paletted.palette.direct() {
        for pair in paletted.packed_long_array.chunks_exact(2) {
            lut[u16::from_le_bytes([pair[0], pair[1]]) as usize] = 1;
        }
        return;
    }
    for &atom in paletted.palette.palette.iter() {
        lut[atom as usize] = 1;
    }
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
    for (atom, &mark) in lut.iter().enumerate() {
        if mark == 1 {
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
    paletted: &PalettedData<UNPACKED_SIZE>,
    lut: &[u32; ATOM_COUNT],
    scratch: &mut ReusablePackScratch,
) -> Result<(), String> {
    if UNPACKED_SIZE > MAX_CELLS {
        return Err(format!(
            "section size {UNPACKED_SIZE} exceeds pack scratch capacity"
        ));
    }
    if !paletted.palette.direct()
        && paletted.palette.palette.len() <= MAX_INDIRECT_PALETTE_SIZE
        && matches!(paletted.palette.bits_per_entry, 1 | 2 | 4 | 8)
    {
        extract_indices(
            paletted.palette.bits_per_entry,
            &paletted.packed_long_array,
            &mut scratch.values[..UNPACKED_SIZE],
        );
        return emit_from_indices(
            streams,
            domain,
            UNPACKED_SIZE,
            &paletted.palette.palette,
            lut,
            scratch,
        );
    }
    if paletted.palette.direct() {
        extract_direct_atoms(
            &paletted.packed_long_array,
            &mut scratch.wide[..UNPACKED_SIZE],
        );
        return emit_from_atoms(
            streams,
            domain,
            UNPACKED_SIZE,
            Some(&paletted.packed_long_array),
            lut,
            scratch,
        );
    }
    extract_atoms(paletted, &mut scratch.wide[..UNPACKED_SIZE]);
    emit_from_atoms(streams, domain, UNPACKED_SIZE, None, lut, scratch)
}

fn emit_from_indices(
    streams: &mut Streams,
    domain: Domain,
    cells: usize,
    source_palette: &[u16],
    lut: &[u32; ATOM_COUNT],
    scratch: &mut ReusablePackScratch,
) -> Result<(), String> {
    let ReusablePackScratch {
        values,
        hist,
        distinct,
        order,
        rank_atoms,
        remap,
        ..
    } = scratch;
    let values = &values[..cells];
    hist.fill(0);
    for &value in values {
        hist[value as usize] += 1;
    }
    let mut distinct_len = 0usize;
    for (source, &count) in hist.iter().enumerate().take(source_palette.len()) {
        if count != 0 {
            distinct[distinct_len] = source as u8;
            distinct_len += 1;
        }
    }
    if distinct_len == 1 {
        return emit_single(
            streams,
            domain,
            cells,
            source_palette[distinct[0] as usize],
            lut,
        );
    }
    order.clear();
    order.extend(0..distinct_len as u16);
    order.sort_unstable_by_key(|&i| {
        let source = distinct[i as usize];
        (std::cmp::Reverse(hist[source as usize]), source)
    });
    rank_atoms.clear();
    for &i in order.iter() {
        let source = distinct[i as usize] as usize;
        remap[source] = rank_atoms.len() as u8;
        rank_atoms.push(source_palette[source]);
    }
    finish_indirect(streams, domain, cells, distinct_len, lut, scratch)
}

fn emit_from_atoms(
    streams: &mut Streams,
    domain: Domain,
    cells: usize,
    copy_source: Option<&[u8]>,
    lut: &[u32; ATOM_COUNT],
    scratch: &mut ReusablePackScratch,
) -> Result<(), String> {
    scratch.seen_gen = scratch.seen_gen.wrapping_add(1);
    if scratch.seen_gen == 0 {
        scratch.seen.fill(0);
        scratch.seen_gen = 1;
    }
    let mark = scratch.seen_gen;
    scratch.atoms.clear();
    for &atom in &scratch.wide[..cells] {
        if scratch.seen[atom as usize] == mark {
            continue;
        }
        scratch.seen[atom as usize] = mark;
        if scratch.atoms.len() == MAX_INDIRECT_PALETTE_SIZE {
            return emit_direct(streams, domain, cells, copy_source, scratch);
        }
        scratch.indices[atom as usize] = scratch.atoms.len() as u8;
        scratch.atoms.push(atom);
    }
    if scratch.atoms.len() == 1 {
        return emit_single(streams, domain, cells, scratch.atoms[0], lut);
    }
    scratch.counts.clear();
    scratch.counts.resize(scratch.atoms.len(), 0);
    for &atom in &scratch.wide[..cells] {
        scratch.counts[scratch.indices[atom as usize] as usize] += 1;
    }
    scratch.order.clear();
    scratch.order.extend(0..scratch.atoms.len() as u16);
    scratch.order.sort_unstable_by_key(|&i| {
        let i = i as usize;
        (std::cmp::Reverse(scratch.counts[i]), scratch.atoms[i])
    });
    scratch.rank_atoms.clear();
    for &i in scratch.order.iter() {
        let i = i as usize;
        scratch.remap[i] = scratch.rank_atoms.len() as u8;
        scratch.rank_atoms.push(scratch.atoms[i]);
    }
    {
        let ReusablePackScratch {
            values,
            wide,
            indices,
            ..
        } = scratch;
        let values = &mut values[..cells];
        for (value, atom) in values.iter_mut().zip(wide[..cells].iter()) {
            *value = indices[*atom as usize];
        }
    }
    finish_indirect(streams, domain, cells, scratch.atoms.len(), lut, scratch)
}

fn finish_indirect(
    streams: &mut Streams,
    domain: Domain,
    cells: usize,
    distinct_len: usize,
    lut: &[u32; ATOM_COUNT],
    scratch: &mut ReusablePackScratch,
) -> Result<(), String> {
    let target_bits = bits_per_entry(distinct_len);
    let bucket = domain.bucket(target_bits);
    streams.bucket_counts[bucket] += 1;
    put_u16_le(&mut streams.local_palettes, distinct_len as u16);
    for &atom in &scratch.rank_atoms[..distinct_len] {
        let index = lut[atom as usize];
        if index == u32::MAX {
            return Err(format!("global palette miss for atom {atom:#06x}"));
        }
        put_u16_le(&mut streams.local_palettes, index as u16);
    }
    let ReusablePackScratch {
        values, remap, out, ..
    } = scratch;
    let values = &mut values[..cells];
    remap_values(values, remap);
    let packed_len = layout::packed_byte_len(cells, target_bits);
    pack_values(target_bits, values, &mut out[..packed_len]);
    put_bytes(&mut streams.buckets[bucket], &out[..packed_len]);
    Ok(())
}

fn emit_single(
    streams: &mut Streams,
    domain: Domain,
    cells: usize,
    atom: u16,
    lut: &[u32; ATOM_COUNT],
) -> Result<(), String> {
    let index = lut[atom as usize];
    if index == u32::MAX {
        return Err(format!("global palette miss for atom {atom:#06x}"));
    }
    put_u16_le(&mut streams.local_palettes, 1);
    put_u16_le(&mut streams.local_palettes, index as u16);
    let bucket = domain.bucket(1);
    let zero_bytes = layout::packed_byte_len(cells, 1);
    let packed = &mut streams.buckets[bucket];
    packed.resize(packed.len() + zero_bytes, 0);
    streams.bucket_counts[bucket] += 1;
    Ok(())
}

fn emit_direct(
    streams: &mut Streams,
    domain: Domain,
    cells: usize,
    copy_source: Option<&[u8]>,
    scratch: &mut ReusablePackScratch,
) -> Result<(), String> {
    let bucket = domain.bucket(16);
    streams.bucket_counts[bucket] += 1;
    put_u16_le(&mut streams.local_palettes, 0);
    match copy_source {
        Some(bytes) => put_bytes(&mut streams.buckets[bucket], bytes),
        None => {
            let packed_len = cells * 2;
            pack_atoms_le(&scratch.wide[..cells], &mut scratch.wide_out[..packed_len]);
            put_bytes(
                &mut streams.buckets[bucket],
                &scratch.wide_out[..packed_len],
            );
        }
    }
    Ok(())
}
