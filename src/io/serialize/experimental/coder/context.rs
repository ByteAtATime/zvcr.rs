use super::super::rans::*;
use crate::definitions::{REGION_SIDELENGTH_SEGMENTS, SECTION_SIZE_BLOCKS, SEGMENTS_PER_REGION};
use std::sync::OnceLock;

const CTX_BITS: u32 = 22;
const CTX_COUNT: usize = 1 << CTX_BITS;
const CTX_LIST_CAP: usize = 96;
const LEN_BUCKETS: usize = CTX_LIST_CAP + 1;

pub(crate) const ROW_STRIDE: usize = 512 * 512;
const COL_STRIDE: usize = 512;
const RUN_THRESHOLD: usize = 384;

static SAT_ITER: OnceLock<Box<[[u16; ANS_M as usize]; 14]>> = OnceLock::new();

fn sat_iter_tables() -> &'static [[u16; ANS_M as usize]; 14] {
    SAT_ITER.get_or_init(|| {
        let mut tables = Box::new([[0u16; ANS_M as usize]; 14]);
        for p in 1..ANS_M as usize {
            tables[0][p] = saturated_zero_step(p as u16);
        }
        for i in 1..14 {
            let prev = tables[i - 1];
            for p in 1..ANS_M as usize {
                tables[i][p] = prev[prev[p] as usize];
            }
        }
        tables
    })
}

#[inline]
fn sat_iter_apply(mut p: u16, mut k: usize) -> u16 {
    let tables = sat_iter_tables();
    let mut i = 0;
    while k > 0 {
        if k & 1 != 0 {
            p = tables[i][p as usize];
        }
        k >>= 1;
        i += 1;
    }
    p
}

pub(crate) fn write_varint(buf: &mut Vec<u8>, mut v: u32) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

pub(crate) fn read_varint(bytes: &[u8], mut pos: usize) -> Result<(u32, usize), String> {
    let mut v = 0u32;
    let mut shift = 0;
    loop {
        let byte = *bytes
            .get(pos)
            .ok_or_else(|| "run descriptor truncated".to_string())?;
        pos += 1;
        v |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok((v, pos));
        }
        shift += 7;
        if shift >= 35 {
            return Err("run descriptor varint overflow".to_string());
        }
    }
}

#[inline]
fn hash_context(west: u16, down: u16, north: u16, down2: u16) -> usize {
    let key = west as u64
        | (down as u64) << 16
        | (north as u64) << 32
        | (down2 as u64) << 48;
    ((key.wrapping_mul(0x9E3779B97F4A7C15)) >> (64 - CTX_BITS)) as usize
}

#[inline]
fn neighbor_agreement(west: u16, down: u16, north: u16) -> usize {
    if west == down && down == north {
        2
    } else if west == down || west == north || down == north {
        1
    } else {
        0
    }
}

#[inline]
fn bucket_for_length(n: usize) -> usize {
    n.min(CTX_LIST_CAP)
}

struct OverflowList {
    syms: Vec<u16>,
    freqs: Vec<u32>,
}

struct ContextSlot {
    rank0_sym: u16,
    len: u8,
    rank0_freq: u32,
    hits: u32,
    chance0: BitChance,
    chances: [BitChance; 3],
    syms: [u16; 3],
    freqs: [u32; 3],
    overflow: Option<Box<OverflowList>>,
}

impl ContextSlot {
    fn empty() -> Self {
        Self {
            rank0_sym: 0,
            len: 0,
            rank0_freq: 0,
            hits: 0,
            chance0: BitChance::fresh(),
            chances: [BitChance::fresh(); 3],
            syms: [0; 3],
            freqs: [0; 3],
            overflow: None,
        }
    }

    #[inline]
    fn sym(&self, rank: usize) -> u16 {
        if rank == 0 {
            self.rank0_sym
        } else if rank <= 3 {
            self.syms[rank - 1]
        } else {
            self.overflow.as_ref().unwrap().syms[rank - 4]
        }
    }

    #[inline]
    fn freq(&self, rank: usize) -> u32 {
        if rank == 0 {
            self.rank0_freq
        } else if rank <= 3 {
            self.freqs[rank - 1]
        } else {
            self.overflow.as_ref().unwrap().freqs[rank - 4]
        }
    }

    #[inline]
    fn set_sym(&mut self, rank: usize, v: u16) {
        if rank == 0 {
            self.rank0_sym = v;
        } else if rank <= 3 {
            self.syms[rank - 1] = v;
        } else {
            self.overflow.as_mut().unwrap().syms[rank - 4] = v;
        }
    }

    #[inline]
    fn set_freq(&mut self, rank: usize, v: u32) {
        if rank == 0 {
            self.rank0_freq = v;
        } else if rank <= 3 {
            self.freqs[rank - 1] = v;
        } else {
            self.overflow.as_mut().unwrap().freqs[rank - 4] = v;
        }
    }
}

struct ContextModel {
    heads: Vec<u32>,
    entries: Vec<ContextSlot>,
    global_syms: Vec<u16>,
    global_freqs: Vec<u32>,
    global_positions: Vec<i32>,
    global_hits: u32,
    rank_coders: Vec<Box<NzCoder>>,
    global_rank_coder: Box<NzCoder>,
    literal_coder: Box<NzCoder>,
    touched_contexts: Vec<u32>,
}

impl ContextModel {
    fn new() -> Self {
        Self {
            heads: vec![0u32; CTX_COUNT],
            entries: vec![ContextSlot::empty()],
            global_syms: Vec::with_capacity(256),
            global_freqs: Vec::with_capacity(256),
            global_positions: vec![-1i32; 65536],
            global_hits: 0,
            rank_coders: (0..(3 * LEN_BUCKETS)).map(|_| NzCoder::new()).collect(),
            global_rank_coder: NzCoder::new(),
            literal_coder: NzCoder::new(),
            touched_contexts: Vec::new(),
        }
    }

    #[inline]
    fn ctx_idx(&mut self, ctx: usize) -> usize {
        let h = self.heads[ctx];
        if h != 0 {
            h as usize
        } else {
            let i = self.entries.len();
            self.entries.push(ContextSlot::empty());
            self.heads[ctx] = i as u32;
            self.touched_contexts.push(ctx as u32);
            i
        }
    }

    fn reset(&mut self) {
        for &ctx in &self.touched_contexts {
            let c = ctx as usize;
            let ci = self.heads[c] as usize;
            self.heads[c] = 0;
            self.entries[ci].len = 0;
            self.entries[ci].overflow = None;
            self.entries[ci].hits = 0;
            let fresh = BitChance::fresh();
            self.entries[ci].chance0 = fresh;
            self.entries[ci].chances = [fresh, fresh, fresh];
        }
        self.touched_contexts.clear();
        self.entries.truncate(1);

        for &sym in &self.global_syms {
            self.global_positions[sym as usize] = -1;
        }
        self.global_syms.clear();
        self.global_freqs.clear();
        self.global_hits = 0;

        for coder in self.rank_coders.iter_mut() {
            coder.reset();
        }
        self.global_rank_coder.reset();
        self.literal_coder.reset();
    }

    #[inline]
    fn bump_context(&mut self, ci: usize, mut rank: usize) {
        self.entries[ci].hits += 1;
        if rank == 0 {
            self.entries[ci].rank0_freq += 1;
            return;
        }
        let hysteresis = 1 + self.entries[ci].hits / 32;
        let new_freq = self.entries[ci].freq(rank) + 1;
        self.entries[ci].set_freq(rank, new_freq);
        let curr_freq = self.entries[ci].freq(rank) + hysteresis;
        while rank > 1 && self.entries[ci].freq(rank - 1) <= curr_freq {
            let hi_sym = self.entries[ci].sym(rank);
            let hi_freq = self.entries[ci].freq(rank);
            let lo_sym = self.entries[ci].sym(rank - 1);
            let lo_freq = self.entries[ci].freq(rank - 1);
            self.entries[ci].set_freq(rank, lo_freq);
            self.entries[ci].set_sym(rank, lo_sym);
            self.entries[ci].set_freq(rank - 1, hi_freq);
            self.entries[ci].set_sym(rank - 1, hi_sym);
            rank -= 1;
        }
        if rank == 1 && self.entries[ci].rank0_freq <= curr_freq {
            let old_f1 = self.entries[ci].freq(1);
            let old_s1 = self.entries[ci].sym(1);
            let rank0_freq = self.entries[ci].rank0_freq;
            let rank0_sym = self.entries[ci].rank0_sym;
            self.entries[ci].set_freq(1, rank0_freq);
            self.entries[ci].set_sym(1, rank0_sym);
            self.entries[ci].rank0_freq = old_f1;
            self.entries[ci].rank0_sym = old_s1;
        }
    }

    #[inline]
    fn bump_global(&mut self, mut g: usize) {
        self.global_freqs[g] += 1;
        self.global_hits += 1;
        let curr_freq = self.global_freqs[g] + (1 + self.global_hits / 32);
        while g > 0 && self.global_freqs[g - 1] <= curr_freq {
            self.global_freqs.swap(g, g - 1);
            self.global_syms.swap(g, g - 1);
            self.global_positions[self.global_syms[g] as usize] = g as i32;
            self.global_positions[self.global_syms[g - 1] as usize] = (g - 1) as i32;
            g -= 1;
        }
    }

    fn insert_context(&mut self, ci: usize, sym: u16) {
        let len = self.entries[ci].len as usize;
        if len == 0 {
            self.entries[ci].rank0_sym = sym;
            self.entries[ci].rank0_freq = 1;
            self.entries[ci].len = 1;
            return;
        }
        if len < CTX_LIST_CAP {
            let new_rank = len;
            if new_rank <= 3 {
                self.entries[ci].set_sym(new_rank, sym);
                self.entries[ci].set_freq(new_rank, 1);
            } else {
                if self.entries[ci].overflow.is_none() {
                    self.entries[ci].overflow = Some(Box::new(OverflowList {
                        syms: Vec::new(),
                        freqs: Vec::new(),
                    }));
                }
                let overflow = self.entries[ci].overflow.as_mut().unwrap();
                overflow.syms.push(sym);
                overflow.freqs.push(1);
            }
            self.entries[ci].len = (len + 1) as u8;
            return;
        }
        let mut coldest = 0usize;
        let mut coldest_freq = self.entries[ci].rank0_freq;
        for i in 1..len {
            let f = self.entries[ci].freq(i);
            if f < coldest_freq {
                coldest = i;
                coldest_freq = f;
            }
        }
        if coldest == 0 {
            self.entries[ci].rank0_sym = sym;
            self.entries[ci].rank0_freq = 1;
        } else {
            self.entries[ci].set_sym(coldest, sym);
            self.entries[ci].set_freq(coldest, 1);
        }
    }

    fn encode_voxel_with_ctx(
        &mut self,
        recs: &mut Vec<BitRec>,
        ctx: usize,
        west: u16,
        down: u16,
        north: u16,
        sym: u16,
        local_sym: u16,
    ) -> (usize, bool) {
        let ci = self.ctx_idx(ctx);
        let n_syms = self.entries[ci].len as usize;
        let rank0_match = n_syms > 0 && self.entries[ci].rank0_sym == sym;
        if n_syms > 0
            && self.try_encode_rank(recs, ci, n_syms, west, down, north, sym, rank0_match)
        {
            return (ci, rank0_match);
        }

        if n_syms > 0 {
            let agr = neighbor_agreement(west, down, north);
            let nz_idx = agr * LEN_BUCKETS + bucket_for_length(n_syms);
            let escape_val = if n_syms <= 3 { 0 } else { (n_syms - 4) as u32 };
            self.rank_coders[nz_idx].encode(recs, escape_val);
        }

        self.encode_global(recs, sym, local_sym);
        self.insert_context(ci, sym);
        (ci, false)
    }

    #[inline]
    fn advance_run_zero(&mut self, ci: usize, steps: usize) {
        for _ in 0..steps {
            self.entries[ci].chance0.update(0, ZERO_LIMIT, ZERO_DELTA);
            self.bump_context(ci, 0);
        }
    }

    #[inline]
    fn record_run_zero(&mut self, recs: &mut Vec<BitRec>, ci: usize) {
        self.entries[ci].chance0.record_bit(recs, 0, ZERO_LIMIT, ZERO_DELTA);
        self.bump_context(ci, 0);
    }

    fn try_encode_rank(
        &mut self,
        recs: &mut Vec<BitRec>,
        ci: usize,
        n_syms: usize,
        west: u16,
        down: u16,
        north: u16,
        sym: u16,
        rank0_match: bool,
    ) -> bool {
        if rank0_match {
            self.entries[ci].chance0.record_bit(recs, 0, ZERO_LIMIT, ZERO_DELTA);
            self.bump_context(ci, 0);
            return true;
        }
        self.entries[ci].chance0.record_bit(recs, 1, ZERO_LIMIT, ZERO_DELTA);

        for rank in 1..n_syms.min(4) {
            let matched = self.entries[ci].sym(rank) == sym;
            self.entries[ci].chances[rank - 1].record_bit(
                recs,
                if matched { 0 } else { 1 },
                ZERO_LIMIT,
                ZERO_DELTA,
            );
            if matched {
                self.bump_context(ci, rank);
                return true;
            }
        }

        for rank in 4..n_syms {
            if self.entries[ci].sym(rank) == sym {
                let agr = neighbor_agreement(west, down, north);
                let nz_idx = agr * LEN_BUCKETS + bucket_for_length(n_syms);
                self.rank_coders[nz_idx].encode(recs, (rank - 4) as u32);
                self.bump_context(ci, rank);
                return true;
            }
        }

        false
    }

    fn encode_global(&mut self, recs: &mut Vec<BitRec>, sym: u16, local_sym: u16) {
        let g = self.global_positions[sym as usize];
        if g >= 0 {
            self.global_rank_coder.encode(recs, g as u32);
            self.bump_global(g as usize);
            return;
        }
        self.global_rank_coder
            .encode(recs, self.global_syms.len() as u32);
        self.literal_coder.encode(recs, local_sym as u32);
        let new_idx = self.global_syms.len();
        self.global_positions[sym as usize] = new_idx as i32;
        self.global_syms.push(sym);
        self.global_freqs.push(1);
    }

    fn decode_voxel(
        &mut self,
        dec: &mut RansDecoder,
        west: u16,
        down: u16,
        north: u16,
        down2: u16,
        palette: &[u16],
    ) -> Result<u16, String> {
        let ctx = hash_context(west, down, north, down2);
        let ci = self.ctx_idx(ctx);
        let n_syms = self.entries[ci].len as usize;

        if n_syms > 0 {
            if let Some(sym) = self.try_decode_rank(dec, ci, n_syms, west, down, north)? {
                return Ok(sym);
            }
        }

        let sym = self.decode_global(dec, palette)?;
        self.insert_context(ci, sym);
        Ok(sym)
    }

    fn try_decode_rank(
        &mut self,
        dec: &mut RansDecoder,
        ci: usize,
        n_syms: usize,
        west: u16,
        down: u16,
        north: u16,
    ) -> Result<Option<u16>, String> {
        if self.entries[ci].chance0.decode_bit(dec, ZERO_LIMIT, ZERO_DELTA) == 0 {
            let sym = self.entries[ci].rank0_sym;
            self.bump_context(ci, 0);
            return Ok(Some(sym));
        }

        for rank in 1..n_syms.min(4) {
            if self.entries[ci].chances[rank - 1].decode_bit(dec, ZERO_LIMIT, ZERO_DELTA) == 0 {
                let sym = self.entries[ci].sym(rank);
                self.bump_context(ci, rank);
                return Ok(Some(sym));
            }
        }

        let agr = neighbor_agreement(west, down, north);
        let nz_idx = agr * LEN_BUCKETS + bucket_for_length(n_syms);
        let q = self.rank_coders[nz_idx].decode(dec);
        let escape_rank = if n_syms > 3 { n_syms - 4 } else { 0 };

        if q as usize == escape_rank {
            return Ok(None);
        }

        let r = q as usize + 4;
        if r >= n_syms {
            return Err(format!(
                "context decode rank {r} out of range for ctx list len {n_syms}"
            ));
        }
        let sym = self.entries[ci].sym(r);
        self.bump_context(ci, r);
        Ok(Some(sym))
    }

    fn decode_global(&mut self, dec: &mut RansDecoder, palette: &[u16]) -> Result<u16, String> {
        let g = self.global_rank_coder.decode(dec) as usize;
        if g < self.global_syms.len() {
            let sym = self.global_syms[g];
            self.bump_global(g);
            return Ok(sym);
        }
        let lit = self.literal_coder.decode(dec) as usize;
        if lit >= palette.len() {
            return Err(format!(
                "context literal index {lit} out of palette range {}",
                palette.len()
            ));
        }
        let sym = palette[lit];
        let new_idx = self.global_syms.len();
        self.global_positions[sym as usize] = new_idx as i32;
        self.global_syms.push(sym);
        self.global_freqs.push(1);
        Ok(sym)
    }
}

struct Neighbors {
    west: u16,
    down: u16,
    north: u16,
    down2: u16,
}

#[inline]
fn read_neighbors_fast(recon: &[u16], idx: usize) -> Neighbors {
    Neighbors {
        west: recon[idx - 1],
        down: recon[idx - ROW_STRIDE],
        north: recon[idx - COL_STRIDE],
        down2: recon[idx - 2 * ROW_STRIDE],
    }
}

#[inline]
fn read_neighbors_safe(
    recon: &[u16],
    idx: usize,
    x: usize,
    ry: usize,
    rz: usize,
    base_x: usize,
) -> Neighbors {
    Neighbors {
        west: if base_x + x > 0 { recon[idx - 1] } else { 0 },
        down: if ry > 0 { recon[idx - ROW_STRIDE] } else { 0 },
        north: if rz > 0 { recon[idx - COL_STRIDE] } else { 0 },
        down2: if ry > 1 { recon[idx - 2 * ROW_STRIDE] } else { 0 },
    }
}

#[inline(always)]
fn load_neighbors(
    blocks: &[u16],
    src: &NeighborSource,
    x: usize,
    y: usize,
    z: usize,
    idx: usize,
) -> (u16, u16, u16, u16) {
    let west = if x > 0 {
        blocks[idx - 1]
    } else {
        src.neighbor_blocks(-1, 0, 0)
            .map_or(0, |b| b[y * 256 + z * 16 + 15])
    };
    let north = if z > 0 {
        blocks[idx - 16]
    } else {
        src.neighbor_blocks(0, -1, 0)
            .map_or(0, |b| b[y * 256 + 15 * 16 + x])
    };
    let down = if y > 0 {
        blocks[idx - 256]
    } else {
        src.neighbor_blocks(0, 0, -1)
            .map_or(0, |b| b[15 * 256 + z * 16 + x])
    };
    let down2 = if y > 1 {
        blocks[idx - 512]
    } else if y == 1 {
        src.neighbor_blocks(0, 0, -1)
            .map_or(0, |b| b[15 * 256 + z * 16 + x])
    } else {
        src.neighbor_blocks(0, 0, -1)
            .map_or(0, |b| b[14 * 256 + z * 16 + x])
    };
    (west, down, north, down2)
}

pub(crate) struct NeighborSource<'a> {
    cache: &'a [([u16; SECTION_SIZE_BLOCKS], Vec<u16>)],
    pos: &'a [i32],
    section_count: usize,
    cx: usize,
    cz: usize,
    sec_idx: usize,
}

impl<'a> NeighborSource<'a> {
    pub(crate) fn new(
        cache: &'a [([u16; SECTION_SIZE_BLOCKS], Vec<u16>)],
        pos: &'a [i32],
        section_count: usize,
        cx: usize,
        cz: usize,
        sec_idx: usize,
    ) -> Self {
        Self {
            cache,
            pos,
            section_count,
            cx,
            cz,
            sec_idx,
        }
    }

    fn neighbor_blocks(
        &self,
        dcx: i32,
        dcz: i32,
        dsec: i32,
    ) -> Option<&'a [u16; SECTION_SIZE_BLOCKS]> {
        let ncx = self.cx as i32 + dcx;
        let ncz = self.cz as i32 + dcz;
        let nsec = self.sec_idx as i32 + dsec;
        if ncx < 0
            || ncx >= REGION_SIDELENGTH_SEGMENTS as i32
            || ncz < 0
            || ncz >= REGION_SIDELENGTH_SEGMENTS as i32
            || nsec < 0
            || nsec >= self.section_count as i32
        {
            return None;
        }
        let nseg = ncx as usize * REGION_SIDELENGTH_SEGMENTS + ncz as usize;
        let idx = self.pos[nsec as usize * SEGMENTS_PER_REGION + nseg];
        if idx < 0 {
            return None;
        }
        Some(&self.cache[idx as usize].0)
    }
}

pub(crate) struct ContextCodec {
    model: ContextModel,
    recon: Option<Vec<u16>>,
    recs: Vec<BitRec>,
    runs: Vec<(u16, u16)>,
    run_len: usize,
    run_sym: u16,
    run_ci: usize,
    run_ctx: usize,
    run_start_idx: usize,
    run_rec_start: usize,
    p_commit: u16,
    committed_len: usize,
}

impl ContextCodec {
    pub(crate) fn new(section_count: usize) -> Self {
        Self {
            model: ContextModel::new(),
            recon: Some(vec![0u16; section_count * 16 * ROW_STRIDE]),
            recs: Vec::with_capacity(32768),
            runs: Vec::new(),
            run_len: 0,
            run_sym: 0,
            run_ci: 0,
            run_ctx: 0,
            run_start_idx: 0,
            run_rec_start: 0,
            p_commit: 0,
            committed_len: 0,
        }
    }

    pub(crate) fn new_encoder(_section_count: usize) -> Self {
        Self {
            model: ContextModel::new(),
            recon: None,
            recs: Vec::with_capacity(32768),
            runs: Vec::new(),
            run_len: 0,
            run_sym: 0,
            run_ci: 0,
            run_ctx: 0,
            run_start_idx: 0,
            run_rec_start: 0,
            p_commit: 0,
            committed_len: 0,
        }
    }

    pub(crate) fn reset(&mut self, section_count: usize) {
        self.model.reset();
        if let Some(recon) = self.recon.as_mut() {
            let needed = section_count * 16 * ROW_STRIDE;
            if recon.len() < needed {
                recon.resize(needed, 0);
            }
            recon[..needed].fill(0);
        }
    }

    pub(crate) fn write_recon(&mut self, blocks: &[u16], cx: usize, cz: usize, sec_idx: usize) {
        let recon = self.recon.as_mut().unwrap();
        let base_x = cx * 16;
        let base_y = sec_idx * 16;
        let base_z = cz * 16;
        for y in 0..16usize {
            let ry = base_y + y;
            for z in 0..16usize {
                let rz = base_z + z;
                let recon_base = ry * ROW_STRIDE + rz * COL_STRIDE + base_x;
                let block_base = y * 256 + z * 16;
                recon[recon_base..recon_base + 16]
                    .copy_from_slice(&blocks[block_base..block_base + 16]);
            }
        }
    }

    pub(crate) fn encode_section(
        &mut self,
        blocks: &[u16],
        val_to_local: &[u16],
        src: &NeighborSource,
    ) -> Vec<u8> {
        self.recs.clear();
        self.runs.clear();
        self.run_len = 0;
        self.run_sym = 0;
        self.run_ci = 0;
        self.run_ctx = 0;
        self.run_start_idx = 0;
        self.run_rec_start = 0;
        self.p_commit = 0;
        self.committed_len = 0;

        for y in 0..16usize {
            for z in 0..16usize {
                let block_base = y * 256 + z * 16;
                let mut x = 0usize;
                while x < 16 {
                    let idx = block_base + x;
                    let sym = blocks[idx];
                    let nbr = load_neighbors(blocks, src, x, y, z, idx);
                    let ctx = hash_context(nbr.0, nbr.1, nbr.2, nbr.3);
                    if self.run_len > 0 && sym == self.run_sym && ctx == self.run_ctx {
                        loop {
                            if self.run_len < RUN_THRESHOLD {
                                self.model.record_run_zero(&mut self.recs, self.run_ci);
                            } else if self.run_len == RUN_THRESHOLD {
                                self.p_commit = self.model.entries[self.run_ci]
                                    .chance0
                                    .prob();
                                self.committed_len = 1;
                            } else {
                                self.committed_len += 1;
                            }
                            self.run_len += 1;
                            x += 1;
                            if x >= 16 {
                                break;
                            }
                            let idx = block_base + x;
                            if blocks[idx] != self.run_sym {
                                break;
                            }
                            let nbr = load_neighbors(blocks, src, x, y, z, idx);
                            if hash_context(nbr.0, nbr.1, nbr.2, nbr.3) != self.run_ctx {
                                break;
                            }
                        }
                        continue;
                    }
                    if self.run_len > 0 {
                        self.end_run();
                        self.run_len = 0;
                    }

                    let (ci, was_rank0) = self.model.encode_voxel_with_ctx(
                        &mut self.recs,
                        ctx,
                        nbr.0,
                        nbr.1,
                        nbr.2,
                        sym,
                        val_to_local[sym as usize],
                    );
                    if was_rank0 {
                        self.run_len = 1;
                        self.run_sym = sym;
                        self.run_ci = ci;
                        self.run_ctx = ctx;
                        self.run_start_idx = idx;
                        self.run_rec_start = self.recs.len() - 1;
                        self.committed_len = 0;
                    }
                    x += 1;
                }
            }
        }

        if self.run_len > 0 {
            self.end_run();
        }

        let mut out = Vec::with_capacity(self.runs.len() * 3 + 8);
        write_varint(&mut out, self.runs.len() as u32);
        for &(start, len) in &self.runs {
            write_varint(&mut out, start as u32);
            write_varint(&mut out, len as u32);
        }
        out.extend_from_slice(&flush_bit_recs(&self.recs));
        out
    }

    #[inline]
    fn end_run(&mut self) {
        if self.run_len >= RUN_THRESHOLD {
            if self.committed_len > 0 {
                let p = self.p_commit;
                let k = self.committed_len;
                self.model.entries[self.run_ci].chance0.set_prob(sat_iter_apply(p, k));
                self.model.entries[self.run_ci].hits += k as u32;
                self.model.entries[self.run_ci].rank0_freq += k as u32;
            }
            self.recs.truncate(self.run_rec_start);
            self.runs.push((self.run_start_idx as u16, self.run_len as u16));
        }
    }

    pub(crate) fn decode_section(
        &mut self,
        ans_bytes: &[u8],
        runs: &[(u16, u16)],
        palette: &[u16],
        cx: usize,
        cz: usize,
        sec_idx: usize,
    ) -> Result<[u16; 4096], String> {
        let mut dec =
            RansDecoder::new(ans_bytes).map_err(|e| format!("rANS decode error: {e}"))?;

        let recon = self.recon.as_mut().unwrap();
        let mut result = [0u16; 4096];
        let base_x = cx * 16;
        let base_y = sec_idx * 16;
        let base_z = cz * 16;

        let mut run_ptr = 0usize;
        let mut run_remaining = 0usize;
        let mut run_ci = 0usize;
        let mut run_sym = 0u16;

        for y in 0..16usize {
            let ry = base_y + y;
            for z in 0..16usize {
                let rz = base_z + z;
                let row_base = ry * ROW_STRIDE + rz * COL_STRIDE + base_x;
                let block_base = y * 256 + z * 16;

                let fast = base_x > 0 && ry > 1 && rz > 0;
                for x in 0..16usize {
                    let idx = block_base + x;
                    let row_idx = row_base + x;
                    let sym = if run_remaining > 0 {
                        self.model.advance_run_zero(run_ci, 1);
                        run_remaining -= 1;
                        run_sym
                    } else if run_ptr < runs.len() && runs[run_ptr].0 as usize == idx {
                        let nb = if fast {
                            read_neighbors_fast(recon, row_idx)
                        } else {
                            read_neighbors_safe(recon, row_idx, x, ry, rz, base_x)
                        };
                        let ctx = hash_context(nb.west, nb.down, nb.north, nb.down2);
                        let ci = self.model.ctx_idx(ctx);
                        if self.model.entries[ci].len == 0 {
                            return Err(format!("run starts on empty context at voxel {idx}"));
                        }
                        let sym = self.model.entries[ci].rank0_sym;
                        self.model.advance_run_zero(ci, 1);
                        run_remaining = runs[run_ptr].1 as usize - 1;
                        run_ci = ci;
                        run_sym = sym;
                        run_ptr += 1;
                        sym
                    } else {
                        let nb = if fast {
                            read_neighbors_fast(recon, row_idx)
                        } else {
                            read_neighbors_safe(recon, row_idx, x, ry, rz, base_x)
                        };
                        self.model.decode_voxel(
                            &mut dec,
                            nb.west,
                            nb.down,
                            nb.north,
                            nb.down2,
                            palette,
                        )?
                    };
                    result[idx] = sym;
                    recon[row_idx] = sym;
                }
            }
        }
        Ok(result)
    }
}
