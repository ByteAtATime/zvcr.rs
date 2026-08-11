use super::super::rans::*;

const CTX_BITS: u32 = 22;
const CTX_COUNT: usize = 1 << CTX_BITS;
const CTX_LIST_CAP: usize = 96;
const LEN_BUCKETS: usize = CTX_LIST_CAP + 1;

pub(crate) const ROW_STRIDE: usize = 512 * 512;
const COL_STRIDE: usize = 512;

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

struct ContextModel {
    ctx_syms: Vec<Vec<u16>>,
    ctx_freq: Vec<Vec<u32>>,
    ctx_hits: Vec<u32>,
    ctx_rank0_sym: Vec<u16>,
    ctx_rank0_freq: Vec<u32>,
    global_syms: Vec<u16>,
    global_freqs: Vec<u32>,
    global_positions: Vec<i32>,
    global_hits: u32,
    rank_chances: [Vec<BitChance>; 4],
    rank_coders: Vec<Box<NzCoder>>,
    global_rank_coder: Box<NzCoder>,
    literal_coder: Box<NzCoder>,
    touched_contexts: Vec<u32>,
}

impl ContextModel {
    fn new() -> Self {
        let fresh = BitChance::fresh();
        Self {
            ctx_syms: (0..CTX_COUNT).map(|_| Vec::new()).collect(),
            ctx_freq: (0..CTX_COUNT).map(|_| Vec::new()).collect(),
            ctx_hits: vec![0; CTX_COUNT],
            ctx_rank0_sym: vec![0; CTX_COUNT],
            ctx_rank0_freq: vec![0; CTX_COUNT],
            global_syms: Vec::with_capacity(256),
            global_freqs: Vec::with_capacity(256),
            global_positions: vec![-1i32; 65536],
            global_hits: 0,
            rank_chances: [
                vec![fresh; CTX_COUNT],
                vec![fresh; CTX_COUNT],
                vec![fresh; CTX_COUNT],
                vec![fresh; CTX_COUNT],
            ],
            rank_coders: (0..(3 * LEN_BUCKETS)).map(|_| NzCoder::new()).collect(),
            global_rank_coder: NzCoder::new(),
            literal_coder: NzCoder::new(),
            touched_contexts: Vec::with_capacity(8192),
        }
    }

    fn reset(&mut self) {
        for &ctx in &self.touched_contexts {
            let c = ctx as usize;
            self.ctx_syms[c].clear();
            self.ctx_freq[c].clear();
            self.ctx_hits[c] = 0;
            let fresh = BitChance::fresh();
            for chances in self.rank_chances.iter_mut() {
                chances[c] = fresh;
            }
        }
        self.touched_contexts.clear();

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
    fn bump_context(&mut self, ctx: usize, mut rank: usize) {
        self.ctx_hits[ctx] += 1;
        if rank == 0 {
            self.ctx_rank0_freq[ctx] += 1;
            return;
        }
        let hysteresis = 1 + self.ctx_hits[ctx] / 32;
        self.ctx_freq[ctx][rank] += 1;
        let curr_freq = self.ctx_freq[ctx][rank] + hysteresis;
        while rank > 1 && self.ctx_freq[ctx][rank - 1] <= curr_freq {
            self.ctx_freq[ctx].swap(rank, rank - 1);
            self.ctx_syms[ctx].swap(rank, rank - 1);
            rank -= 1;
        }
        if rank == 1 && self.ctx_rank0_freq[ctx] <= curr_freq {
            let old_f1 = self.ctx_freq[ctx][1];
            let old_s1 = self.ctx_syms[ctx][1];
            self.ctx_freq[ctx][1] = self.ctx_rank0_freq[ctx];
            self.ctx_syms[ctx][1] = self.ctx_rank0_sym[ctx];
            self.ctx_rank0_freq[ctx] = old_f1;
            self.ctx_rank0_sym[ctx] = old_s1;
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

    fn insert_context(&mut self, ctx: usize, sym: u16) {
        let len = self.ctx_syms[ctx].len();
        if len == 0 {
            self.touched_contexts.push(ctx as u32);
            self.ctx_rank0_sym[ctx] = sym;
            self.ctx_rank0_freq[ctx] = 1;
        }
        if len < CTX_LIST_CAP {
            self.ctx_syms[ctx].push(sym);
            self.ctx_freq[ctx].push(1);
            return;
        }
        let mut coldest = 0usize;
        let mut coldest_freq = self.ctx_rank0_freq[ctx];
        for i in 1..len {
            let f = self.ctx_freq[ctx][i];
            if f < coldest_freq {
                coldest = i;
                coldest_freq = f;
            }
        }
        if coldest == 0 {
            self.ctx_rank0_sym[ctx] = sym;
            self.ctx_rank0_freq[ctx] = 1;
            self.ctx_syms[ctx][0] = sym;
        } else {
            self.ctx_syms[ctx][coldest] = sym;
            self.ctx_freq[ctx][coldest] = 1;
        }
    }

    fn encode_voxel(
        &mut self,
        recs: &mut Vec<BitRec>,
        west: u16,
        down: u16,
        north: u16,
        down2: u16,
        sym: u16,
        local_sym: u16,
    ) {
        let ctx = hash_context(west, down, north, down2);
        let n_syms = self.ctx_syms[ctx].len();

        if n_syms > 0 && self.try_encode_rank(recs, ctx, n_syms, west, down, north, sym) {
            return;
        }

        if n_syms > 0 {
            let agr = neighbor_agreement(west, down, north);
            let nz_idx = agr * LEN_BUCKETS + bucket_for_length(n_syms);
            let escape_val = if n_syms <= 3 { 0 } else { (n_syms - 4) as u32 };
            self.rank_coders[nz_idx].encode(recs, escape_val);
        }

        self.encode_global(recs, sym, local_sym);
        self.insert_context(ctx, sym);
    }

    fn try_encode_rank(
        &mut self,
        recs: &mut Vec<BitRec>,
        ctx: usize,
        n_syms: usize,
        west: u16,
        down: u16,
        north: u16,
        sym: u16,
    ) -> bool {
        if self.ctx_rank0_sym[ctx] == sym {
            self.rank_chances[0][ctx].record_bit(recs, 0, ZERO_LIMIT, ZERO_DELTA);
            self.bump_context(ctx, 0);
            return true;
        }
        self.rank_chances[0][ctx].record_bit(recs, 1, ZERO_LIMIT, ZERO_DELTA);

        for rank in 1..n_syms.min(4) {
            let matched = self.ctx_syms[ctx][rank] == sym;
            self.rank_chances[rank][ctx].record_bit(
                recs,
                if matched { 0 } else { 1 },
                ZERO_LIMIT,
                ZERO_DELTA,
            );
            if matched {
                self.bump_context(ctx, rank);
                return true;
            }
        }

        for rank in 4..n_syms {
            if self.ctx_syms[ctx][rank] == sym {
                let agr = neighbor_agreement(west, down, north);
                let nz_idx = agr * LEN_BUCKETS + bucket_for_length(n_syms);
                self.rank_coders[nz_idx].encode(recs, (rank - 4) as u32);
                self.bump_context(ctx, rank);
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
        let n_syms = self.ctx_syms[ctx].len();

        if n_syms > 0 {
            if let Some(sym) = self.try_decode_rank(dec, ctx, n_syms, west, down, north)? {
                return Ok(sym);
            }
        }

        let sym = self.decode_global(dec, palette)?;
        self.insert_context(ctx, sym);
        Ok(sym)
    }

    fn try_decode_rank(
        &mut self,
        dec: &mut RansDecoder,
        ctx: usize,
        n_syms: usize,
        west: u16,
        down: u16,
        north: u16,
    ) -> Result<Option<u16>, String> {
        if self.rank_chances[0][ctx].decode_bit(dec, ZERO_LIMIT, ZERO_DELTA) == 0 {
            let sym = self.ctx_rank0_sym[ctx];
            self.bump_context(ctx, 0);
            return Ok(Some(sym));
        }

        for rank in 1..n_syms.min(4) {
            if self.rank_chances[rank][ctx].decode_bit(dec, ZERO_LIMIT, ZERO_DELTA) == 0 {
                let sym = self.ctx_syms[ctx][rank];
                self.bump_context(ctx, rank);
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
        let sym = self.ctx_syms[ctx][r];
        self.bump_context(ctx, r);
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

pub(crate) struct ContextCodec {
    model: ContextModel,
    recon: Vec<u16>,
    recs: Vec<BitRec>,
}

impl ContextCodec {
    pub(crate) fn new(section_count: usize) -> Self {
        Self {
            model: ContextModel::new(),
            recon: vec![0u16; section_count * 16 * ROW_STRIDE],
            recs: Vec::with_capacity(8192),
        }
    }

    pub(crate) fn reset(&mut self, section_count: usize) {
        self.model.reset();
        let needed = section_count * 16 * ROW_STRIDE;
        if self.recon.len() < needed {
            self.recon.resize(needed, 0);
        }
        self.recon[..needed].fill(0);
    }

    pub(crate) fn write_recon(&mut self, blocks: &[u16], cx: usize, cz: usize, sec_idx: usize) {
        let base_x = cx * 16;
        let base_y = sec_idx * 16;
        let base_z = cz * 16;
        for y in 0..16usize {
            let ry = base_y + y;
            for z in 0..16usize {
                let rz = base_z + z;
                let recon_base = ry * ROW_STRIDE + rz * COL_STRIDE + base_x;
                let block_base = y * 256 + z * 16;
                self.recon[recon_base..recon_base + 16]
                    .copy_from_slice(&blocks[block_base..block_base + 16]);
            }
        }
    }

    pub(crate) fn encode_section(
        &mut self,
        blocks: &[u16],
        val_to_local: &[u16],
        cx: usize,
        cz: usize,
        sec_idx: usize,
    ) -> Vec<u8> {
        let base_x = cx * 16;
        let base_y = sec_idx * 16;
        let base_z = cz * 16;

        self.write_recon(blocks, cx, cz, sec_idx);
        self.recs.clear();

        for y in 0..16usize {
            let ry = base_y + y;
            for z in 0..16usize {
                let rz = base_z + z;
                let row_base = ry * ROW_STRIDE + rz * COL_STRIDE + base_x;
                let block_base = y * 256 + z * 16;

                if base_x > 0 && ry > 1 && rz > 0 {
                    for x in 0..16usize {
                        let idx = row_base + x;
                        let nb = read_neighbors_fast(&self.recon, idx);
                        let sym = blocks[block_base + x];
                        self.model.encode_voxel(
                            &mut self.recs,
                            nb.west,
                            nb.down,
                            nb.north,
                            nb.down2,
                            sym,
                            val_to_local[sym as usize],
                        );
                    }
                } else {
                    for x in 0..16usize {
                        let idx = row_base + x;
                        let nb = read_neighbors_safe(&self.recon, idx, x, ry, rz, base_x);
                        let sym = blocks[block_base + x];
                        self.model.encode_voxel(
                            &mut self.recs,
                            nb.west,
                            nb.down,
                            nb.north,
                            nb.down2,
                            sym,
                            val_to_local[sym as usize],
                        );
                    }
                }
            }
        }

        flush_bit_recs(&self.recs)
    }

    pub(crate) fn decode_section(
        &mut self,
        ans_bytes: &[u8],
        palette: &[u16],
        cx: usize,
        cz: usize,
        sec_idx: usize,
    ) -> Result<[u16; 4096], String> {
        let mut dec =
            RansDecoder::new(ans_bytes).map_err(|e| format!("rANS decode error: {e}"))?;

        let mut result = [0u16; 4096];
        let base_x = cx * 16;
        let base_y = sec_idx * 16;
        let base_z = cz * 16;

        for y in 0..16usize {
            let ry = base_y + y;
            for z in 0..16usize {
                let rz = base_z + z;
                let row_base = ry * ROW_STRIDE + rz * COL_STRIDE + base_x;
                let block_base = y * 256 + z * 16;

                if base_x > 0 && ry > 1 && rz > 0 {
                    for x in 0..16usize {
                        let idx = row_base + x;
                        let nb = read_neighbors_fast(&self.recon, idx);
                        let sym = self.model.decode_voxel(
                            &mut dec,
                            nb.west,
                            nb.down,
                            nb.north,
                            nb.down2,
                            palette,
                        )?;
                        result[block_base + x] = sym;
                        self.recon[idx] = sym;
                    }
                } else {
                    for x in 0..16usize {
                        let idx = row_base + x;
                        let nb = read_neighbors_safe(&self.recon, idx, x, ry, rz, base_x);
                        let sym = self.model.decode_voxel(
                            &mut dec,
                            nb.west,
                            nb.down,
                            nb.north,
                            nb.down2,
                            palette,
                        )?;
                        result[block_base + x] = sym;
                        self.recon[idx] = sym;
                    }
                }
            }
        }
        Ok(result)
    }
}
