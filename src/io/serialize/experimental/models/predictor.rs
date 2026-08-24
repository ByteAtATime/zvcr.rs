use crate::io::serialize::experimental::models::range::{Decoder, Encoder};
use crate::io::serialize::experimental::models::spatial::{SECTION_SIDE, SIDE, Y_STRIDE, Z_STRIDE};

pub const NONE: u16 = u16::MAX;
const FIRST_ORDER: usize = 9;
pub const CHAIN_SLOTS: usize = 12;
pub(super) const CHAIN_ORDER: [usize; CHAIN_SLOTS] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const PRIMARY_VALUE_WEIGHTS: [u32; FIRST_ORDER] = [3, 3, 3, 2, 2, 2, 2, 1, 1];

pub const PROB_MAX: i32 = 4095;
pub const PROB_HALF: u16 = 2048;

pub const MIX_INPUTS: usize = 10;
pub const CONF_BUCKETS: usize = 8;
pub const TREE_INPUTS: usize = 3;
pub const MAX_BIT_DEPTH: usize = 16;

const MIXER_UNIT: i32 = 1 << 16;
pub const PRIMARY_MIXER_SEED_WEIGHT: i32 = MIXER_UNIT / MIX_INPUTS as i32;
pub const TREE_MIXER_SEED_WEIGHT: i32 = MIXER_UNIT / TREE_INPUTS as i32;
const WEIGHT_LIMIT: i32 = 1 << 20;
pub const ADAPT_RATE_SHIFT: i32 = 5;
pub const HEAD_ADAPT_SHIFT: i32 = 4;
const COUNT_LIMIT: u8 = 29;

const COUNT_RATE_SHIFTS: [i32; COUNT_LIMIT as usize + 1] = [
    2, 2, 2, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5,
];

const PRIMARY_TABLE_SIZE: usize = 1 << 18;
pub(super) const PRIMARY_TABLE_MASK: u32 = PRIMARY_TABLE_SIZE as u32 - 1;
pub(super) const CHAIN_TABLE_SIZE: usize = 1 << 16;
pub(super) const CHAIN_TABLE_MASK: u32 = CHAIN_TABLE_SIZE as u32 - 1;
const TREE_BAND_TABLE_SIZE: usize = 1 << 12;
pub(super) const TREE_BAND_TABLE_MASK: u32 = TREE_BAND_TABLE_SIZE as u32 - 1;

// as follows are a bunch of random hex values used as a seed
// they have to be deterministic so that... y'know... the file can be decoded
// changing them is not backwards compatible, but these specific values mean nothing
// (i just took them from random hash functions and stuff lmao)

const HASH_MIX: u32 = 0x9E37_79B1;
const HASH_FINALIZE: u32 = 0x85EB_CA6B;

const DIRECTION_MULTS: [u32; FIRST_ORDER] = [
    0x9E37_79B1,
    0x85EB_CA77,
    0xC2B2_AE3D,
    0x27D4_EB2F,
    0x1656_67B1,
    0x9E37_79B9,
    0x85EB_CA6B,
    0xC2B2_AE35,
    0x27D4_EB27,
];

const FORWARD_NEIGHBORHOOD_SEED: u32 = 0x1F12_3BB5;
const REVERSE_NEIGHBORHOOD_SEED: u32 = 0x2B7E_1516;

#[cfg(test)]
const SUBSET_CONTEXTS: [(u32, &[usize]); 4] = [
    (0x9E37_79B9, &[9, 10, 11]),
    (0x85EB_CA6B, &[3, 6, 7]),
    (0xC2B2_AE35, &[0, 1, 2]),
    (0x27D4_EB2F, &[1, 2]),
];

#[inline(always)]
fn cluster_of(neighbors: &[u16; CHAIN_SLOTS], miss: &[u8], idx: usize) -> u8 {
    let west_miss = if neighbors[0] != NONE {
        unsafe { *miss.get_unchecked(idx - 1) }
    } else {
        0
    };
    let lower_miss = if neighbors[1] != NONE {
        unsafe { *miss.get_unchecked(idx - Y_STRIDE) }
    } else {
        0
    };
    let north_miss = if neighbors[2] != NONE {
        unsafe { *miss.get_unchecked(idx - Z_STRIDE) }
    } else {
        0
    };
    west_miss | (lower_miss << 1) | (north_miss << 2)
}

#[inline]
pub(super) fn combine(hash: u32, value: u32) -> u32 {
    (hash ^ value.wrapping_mul(HASH_MIX))
        .rotate_left(7)
        .wrapping_mul(HASH_FINALIZE)
}

#[inline]
fn primary_contexts(
    neighbors: &[u16; CHAIN_SLOTS],
    primary_value: u32,
    mask: u32,
    run: u32,
    section_y: usize,
    palette_bits: usize,
) -> PrimaryCtx {
    let n = |slot: usize| (neighbors[slot] as u32).wrapping_mul(DIRECTION_MULTS[slot]);
    let mut fwd = FORWARD_NEIGHBORHOOD_SEED;
    let mut rev = REVERSE_NEIGHBORHOOD_SEED;
    let mut distance_two = 0x9E37_79B9;
    let mut diagonal = 0x85EB_CA6B;
    let mut axial = 0xC2B2_AE35;
    let mut lower_pair = 0x27D4_EB2F;

    fwd = combine(fwd, n(0));
    rev = combine(rev, n(8));
    distance_two = combine(distance_two, neighbors[9] as u32);
    rev = combine(rev, n(7));
    lower_pair = combine(lower_pair, neighbors[1] as u32);
    fwd = combine(fwd, n(1));
    diagonal = combine(diagonal, neighbors[3] as u32);
    axial = combine(axial, neighbors[0] as u32);

    rev = combine(rev, n(6));
    distance_two = combine(distance_two, neighbors[10] as u32);
    fwd = combine(fwd, n(2));
    lower_pair = combine(lower_pair, neighbors[2] as u32);
    diagonal = combine(diagonal, neighbors[6] as u32);
    axial = combine(axial, neighbors[1] as u32);

    fwd = combine(fwd, n(3));
    rev = combine(rev, n(5));
    distance_two = combine(distance_two, neighbors[11] as u32);
    diagonal = combine(diagonal, neighbors[7] as u32);
    axial = combine(axial, neighbors[2] as u32);

    rev = combine(rev, n(4));
    fwd = combine(fwd, n(4));

    fwd = combine(fwd, n(5));
    rev = combine(rev, n(3));

    rev = combine(rev, n(2));
    fwd = combine(fwd, n(6));

    fwd = combine(fwd, n(7));
    rev = combine(rev, n(1));

    rev = combine(rev, n(0));
    fwd = combine(fwd, n(8));

    distance_two = combine(distance_two, primary_value);
    diagonal = combine(diagonal, primary_value);
    axial = combine(axial, primary_value);
    lower_pair = combine(lower_pair, primary_value);
    let neighborhood = combine(fwd, primary_value);
    let rotated_neighborhood = combine(rev, primary_value);
    let value_scaled_hash = combine(neighborhood, primary_value.wrapping_mul(3));
    PrimaryCtx {
        idx: [
            neighborhood & PRIMARY_TABLE_MASK,
            rotated_neighborhood & PRIMARY_TABLE_MASK,
            distance_two & PRIMARY_TABLE_MASK,
            (mask << 8) | run.min(255),
            combine(primary_value, mask << 1) & PRIMARY_TABLE_MASK,
            value_scaled_hash & PRIMARY_TABLE_MASK,
            ((section_y as u32) << 5) | palette_bits as u32,
            diagonal & PRIMARY_TABLE_MASK,
            axial & PRIMARY_TABLE_MASK,
            lower_pair & PRIMARY_TABLE_MASK,
        ],
        hash: neighborhood,
    }
}

const fn squash(logit: i32) -> i32 {
    const SIGMOID_TABLE: [i32; 33] = [
        1, 2, 3, 6, 10, 16, 27, 45, 73, 120, 194, 310, 488, 747, 1101, 1546, 2047, 2549, 2994,
        3348, 3607, 3785, 3901, 3975, 4024, 4050, 4068, 4079, 4085, 4089, 4092, 4093, 4094,
    ];
    if logit > 2047 {
        return PROB_MAX;
    }
    if logit < -2047 {
        return 1;
    }
    let weight = logit & 127;
    let segment = (logit >> 7) + 16;
    (SIGMOID_TABLE[segment as usize] * (128 - weight)
        + SIGMOID_TABLE[segment as usize + 1] * weight
        + 64)
        >> 7
}

const fn build_stretch() -> [i32; 4096] {
    let mut stretch = [0i32; 4096];
    let mut prob = 1usize;
    while prob < 4095 {
        let target = prob as i32;
        let (mut lo, mut hi) = (-2047i32, 2047i32);
        while lo < hi {
            let mid = (lo + hi) >> 1;
            if squash(mid) < target {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        stretch[prob] = lo;
        prob += 1;
    }
    stretch[0] = -2047;
    stretch[4095] = 2047;
    stretch
}

static STRETCH: [i32; 4096] = build_stretch();

#[inline]
pub(super) fn adapt(counter: &mut u16, count: &mut u8, bit: u32, cap: i32) {
    let current = *counter as i32;
    let target = if bit != 0 { PROB_MAX } else { 0 };
    let shift = COUNT_RATE_SHIFTS[(*count).min(COUNT_LIMIT) as usize].min(cap);
    *counter = (current + ((target - current) >> shift)) as u16;
    *count = (*count + 1).min(COUNT_LIMIT);
}

#[inline(always)]
pub(super) fn gather_neighbors(
    voxels: &[u16],
    idx: usize,
    x: usize,
    y: usize,
    z: usize,
    neighbors: &mut [u16; CHAIN_SLOTS],
) {
    let west = x > 0;
    let lower = y > 0;
    let north = z > 0;
    let local_x = x % SECTION_SIDE;
    let local_y = y % SECTION_SIDE;
    let east_causal = lower && x + 1 < SIDE && (local_x < SECTION_SIDE - 1 || local_y == 0);
    neighbors[0] = if west { voxels[idx - 1] } else { NONE };
    neighbors[1] = if lower { voxels[idx - Y_STRIDE] } else { NONE };
    neighbors[2] = if north { voxels[idx - Z_STRIDE] } else { NONE };
    neighbors[3] = if west && north {
        voxels[idx - 1 - Z_STRIDE]
    } else {
        NONE
    };
    neighbors[4] = if west && lower {
        voxels[idx - 1 - Y_STRIDE]
    } else {
        NONE
    };
    neighbors[5] = if lower && north {
        voxels[idx - Y_STRIDE - Z_STRIDE]
    } else {
        NONE
    };
    neighbors[6] = if west && lower && north {
        voxels[idx - 1 - Y_STRIDE - Z_STRIDE]
    } else {
        NONE
    };
    neighbors[7] = if east_causal {
        voxels[idx + 1 - Y_STRIDE]
    } else {
        NONE
    };
    neighbors[8] = if east_causal && north {
        voxels[idx + 1 - Y_STRIDE - Z_STRIDE]
    } else {
        NONE
    };
    neighbors[9] = if x > 1 { voxels[idx - 2] } else { NONE };
    neighbors[10] = if z > 1 {
        voxels[idx - 2 * Z_STRIDE]
    } else {
        NONE
    };
    neighbors[11] = if y > 1 {
        voxels[idx - 2 * Y_STRIDE]
    } else {
        NONE
    };
}

#[inline(always)]
pub(super) fn gather_neighbors_fast(
    voxels: &[u16],
    idx: usize,
    east_causal: bool,
    neighbors: &mut [u16; CHAIN_SLOTS],
) {
    neighbors[0] = voxels[idx - 1];
    neighbors[1] = voxels[idx - Y_STRIDE];
    neighbors[2] = voxels[idx - Z_STRIDE];
    neighbors[3] = voxels[idx - 1 - Z_STRIDE];
    neighbors[4] = voxels[idx - 1 - Y_STRIDE];
    neighbors[5] = voxels[idx - Y_STRIDE - Z_STRIDE];
    neighbors[6] = voxels[idx - 1 - Y_STRIDE - Z_STRIDE];
    neighbors[7] = if east_causal {
        voxels[idx + 1 - Y_STRIDE]
    } else {
        NONE
    };
    neighbors[8] = if east_causal {
        voxels[idx + 1 - Y_STRIDE - Z_STRIDE]
    } else {
        NONE
    };
    neighbors[9] = voxels[idx - 2];
    neighbors[10] = voxels[idx - 2 * Z_STRIDE];
    neighbors[11] = voxels[idx - 2 * Y_STRIDE];
}

const fn build_weight_lut() -> [u32; 512] {
    let mut lut = [0u32; 512];
    let mut mask = 0usize;
    while mask < 512 {
        let mut total = 0u32;
        let mut bit = 0usize;
        while bit < FIRST_ORDER {
            if mask & (1 << bit) != 0 {
                total += PRIMARY_VALUE_WEIGHTS[bit];
            }
            bit += 1;
        }
        lut[mask] = total;
        mask += 1;
    }
    lut
}

static WEIGHT_LUT: [u32; 512] = build_weight_lut();

const fn build_key_lut() -> [u32; 512] {
    let mut lut = [0u32; 512];
    let mut mask = 0usize;
    while mask < 512 {
        lut[mask] = (WEIGHT_LUT[mask] << 5) | (mask as u32).leading_zeros();
        mask += 1;
    }
    lut
}

static KEY_LUT: [u32; 512] = build_key_lut();

#[inline(always)]
fn equality_mask(slice: &[u16; FIRST_ORDER], val: u16) -> u32 {
    let mut mask = 0u32;
    for (i, &cell) in slice.iter().enumerate().take(8) {
        mask |= ((cell == val) as u32) << i;
    }
    mask |= ((slice[8] == val) as u32) << 8;
    mask
}

const SUFFIX_WEIGHTS: [u32; FIRST_ORDER + 1] = {
    let mut s = [0u32; FIRST_ORDER + 1];
    let mut i = FIRST_ORDER;
    while i > 0 {
        s[i - 1] = s[i] + PRIMARY_VALUE_WEIGHTS[i - 1];
        i -= 1;
    }
    s
};

#[inline(always)]
fn select_primary_value(neighbors: &[u16; CHAIN_SLOTS]) -> (u16, u32) {
    let slice: &[u16; FIRST_ORDER] = neighbors[..FIRST_ORDER].try_into().unwrap();
    let mut best_val = NONE;
    let mut best_key = 0u32;
    let mut best_mask = 0x1FF;

    for i in 0..FIRST_ORDER {
        if (best_key >> 5) > SUFFIX_WEIGHTS[i] {
            break;
        }
        let val = slice[i];
        if val == NONE {
            continue;
        }

        let val = slice[i];
        let mask = equality_mask(slice, val);
        let key = KEY_LUT[mask as usize];
        if key > best_key {
            best_key = key;
            best_val = val;
            best_mask = mask;
            if mask == 0x1FF {
                break;
            }
        }
    }

    (best_val, best_mask)
}

pub(super) struct PrimaryCtx {
    pub(super) idx: [u32; MIX_INPUTS],
    pub(super) hash: u32,
}

#[inline]
pub fn mix_logits(weights: &[i32], probs: &[u32]) -> u32 {
    let mut dot = 0i64;
    for (weight, &prob) in weights.iter().zip(probs) {
        dot += *weight as i64 * STRETCH[prob as usize] as i64;
    }
    squash((dot >> 16) as i32) as u32
}

#[inline]
pub fn adapt_weights(weights: &mut [i32], probs: &[u32], bit: u32, mixed: u32) {
    let error = bit as i32 * 4096 - mixed as i32;
    for (weight, &prob) in weights.iter_mut().zip(probs) {
        *weight =
            (*weight + ((error * STRETCH[prob as usize]) >> 13)).clamp(-WEIGHT_LIMIT, WEIGHT_LIMIT);
    }
}

#[inline]
pub fn stretch_probs(probs: &[u32; MIX_INPUTS]) -> [i32; MIX_INPUTS] {
    macro_rules! stretched {
        ($i:literal) => {
            unsafe { STRETCH.as_ptr().add(probs[$i] as usize).read_volatile() }
        };
    }
    [
        stretched!(0),
        stretched!(1),
        stretched!(2),
        stretched!(3),
        stretched!(4),
        stretched!(5),
        stretched!(6),
        stretched!(7),
        stretched!(8),
        stretched!(9),
    ]
}

#[inline]
pub fn mix_stretched(weights: &[i32; MIX_INPUTS], stretched: &[i32; MIX_INPUTS]) -> u32 {
    let mut products = [0i32; MIX_INPUTS];
    for k in 0..MIX_INPUTS {
        products[k] = weights[k] * stretched[k];
    }
    let mut dot = 0i64;
    for &product in &products {
        dot += product as i64;
    }
    squash((dot >> 16) as i32) as u32
}

#[inline]
pub fn adapt_weights_stretched(
    weights: &mut [i32; MIX_INPUTS],
    stretched: &[i32; MIX_INPUTS],
    bit: u32,
    mixed: u32,
) {
    let error = bit as i32 * 4096 - mixed as i32;
    let mut updates = [0i32; MIX_INPUTS];
    for k in 0..MIX_INPUTS {
        updates[k] = (error * stretched[k]) >> 13;
    }
    for k in 0..MIX_INPUTS {
        weights[k] = (weights[k] + updates[k]).clamp(-WEIGHT_LIMIT, WEIGHT_LIMIT);
    }
}

pub(super) struct Predictor {
    pub(super) primary: Box<[[u16; PRIMARY_TABLE_SIZE]; MIX_INPUTS]>,
    pub(super) chain: Box<[[u16; CHAIN_TABLE_SIZE]; CHAIN_SLOTS]>,
    pub(super) chain_counts: Box<[[u8; CHAIN_TABLE_SIZE]; CHAIN_SLOTS]>,
    pub(super) tree: Box<[[u16; PRIMARY_TABLE_SIZE]; 2]>,
    pub(super) tree_counts: Box<[[u8; PRIMARY_TABLE_SIZE]; 2]>,
    pub(super) tree_band: Box<[u16]>,
    pub(super) tree_band_counts: Box<[u8]>,
    pub(super) weights: Box<[[i32; MIX_INPUTS]; (MAX_BIT_DEPTH + 1) * CONF_BUCKETS * 8]>,
    pub(super) tree_weights: Box<[[i32; TREE_INPUTS]; MAX_BIT_DEPTH + 1]>,
    pub(super) run: u32,
}

fn filled_tables<T: Copy, const N: usize, const M: usize>(value: T) -> Box<[[T; N]; M]> {
    let mut flat = vec![value; N * M];
    let ptr = flat.as_mut_ptr();
    std::mem::forget(flat);
    unsafe { Box::from_raw(ptr.cast::<[[T; N]; M]>()) }
}

impl Predictor {
    pub(super) fn new() -> Self {
        Self {
            primary: filled_tables::<u16, PRIMARY_TABLE_SIZE, MIX_INPUTS>(PROB_HALF),
            chain: filled_tables::<u16, CHAIN_TABLE_SIZE, CHAIN_SLOTS>(PROB_HALF),
            chain_counts: filled_tables::<u8, CHAIN_TABLE_SIZE, CHAIN_SLOTS>(0u8),
            tree: filled_tables::<u16, PRIMARY_TABLE_SIZE, 2>(PROB_HALF),
            tree_counts: filled_tables::<u8, PRIMARY_TABLE_SIZE, 2>(0u8),
            tree_band: Box::new([PROB_HALF; TREE_BAND_TABLE_SIZE]),
            tree_band_counts: Box::new([0u8; TREE_BAND_TABLE_SIZE]),
            weights: Box::new(
                [[PRIMARY_MIXER_SEED_WEIGHT; MIX_INPUTS]; (MAX_BIT_DEPTH + 1) * CONF_BUCKETS * 8],
            ),
            tree_weights: Box::new([[TREE_MIXER_SEED_WEIGHT; TREE_INPUTS]; MAX_BIT_DEPTH + 1]),
            run: 0,
        }
    }
}

pub(super) struct HeadLite {
    pub(super) primary_value: u16,
    pub(super) mask: u32,
    pub(super) hash: u32,
    pub(super) bit: u32,
}

pub(super) struct HeadMemo {
    key: [u16; CHAIN_SLOTS],
    valid: bool,
    primary_value: u16,
    mask: u32,
    hash: u32,
    idx: [u32; MIX_INPUTS],
}

impl HeadMemo {
    fn new() -> Self {
        Self {
            key: [NONE; CHAIN_SLOTS],
            valid: false,
            primary_value: NONE,
            mask: 0,
            hash: 0,
            idx: [0; MIX_INPUTS],
        }
    }

    #[inline]
    fn matches(&self, neighbors: &[u16; CHAIN_SLOTS]) -> bool {
        macro_rules! slots {
            ($($i:literal),*) => {{
                const COVERED: u32 = 0 $(| 1 << $i)*;
                const ALL: u32 = (1 << CHAIN_SLOTS) - 1;
                const {
                    assert!(
                        COVERED == ALL,
                        "slot list must cover every CHAIN_SLOTS index exactly once"
                    )
                };
                self.valid
                    $(&& unsafe {
                        self.key.as_ptr().add($i).read_volatile()
                            == neighbors.as_ptr().add($i).read_volatile()
                    })*
            }};
        }
        slots!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11)
    }
}

pub(super) struct HeadState {
    pub(super) neighbors: [u16; CHAIN_SLOTS],
    memo: HeadMemo,
}

impl HeadState {
    pub(super) fn new() -> Self {
        Self {
            neighbors: [NONE; CHAIN_SLOTS],
            memo: HeadMemo::new(),
        }
    }
}

#[derive(Copy, Clone)]
pub(super) struct SectionMetadata {
    pub(super) section_y: usize,
    pub(super) palette_bits: usize,
}

impl Predictor {
    #[inline]
    pub(super) fn encode_head(
        &mut self,
        encoder: &mut Encoder,
        section: SectionMetadata,
        state: &mut HeadState,
        truth: u16,
        miss: &mut [u8],
        idx: usize,
    ) -> HeadLite {
        let palette_bits = section.palette_bits;
        let section_y = section.section_y;
        let memo = &mut state.memo;
        let (primary_value, mask, hash);
        let mut idx_ctx;
        if memo.matches(&state.neighbors) {
            primary_value = memo.primary_value;
            mask = memo.mask;
            hash = memo.hash;
            idx_ctx = memo.idx;
            idx_ctx[3] = (mask << 8) | self.run.min(255);
        } else {
            let (pv, m) = select_primary_value(&state.neighbors);
            let ctx = primary_contexts(
                &state.neighbors,
                pv as u32,
                m,
                self.run,
                section_y,
                palette_bits,
            );
            primary_value = pv;
            mask = m;
            hash = ctx.hash;
            idx_ctx = ctx.idx;
            memo.key = state.neighbors;
            memo.valid = true;
            memo.primary_value = pv;
            memo.mask = m;
            memo.hash = ctx.hash;
            memo.idx = ctx.idx;
        }
        let bit = (truth == primary_value) as u32;
        let ctx = idx_ctx;
        let mut probs = [0u32; MIX_INPUTS];
        for k in 0..MIX_INPUTS {
            probs[k] =
                unsafe { *self.primary.get_unchecked(k).get_unchecked(ctx[k] as usize) } as u32;
        }
        let stretched = stretch_probs(&probs);
        let target = if bit != 0 { PROB_MAX } else { 0 };
        for k in 0..MIX_INPUTS {
            let current = probs[k] as i32;
            unsafe {
                *self
                    .primary
                    .get_unchecked_mut(k)
                    .get_unchecked_mut(ctx[k] as usize) =
                    (current + ((target - current) >> HEAD_ADAPT_SHIFT)) as u16;
            }
        }
        let cluster = cluster_of(&state.neighbors, miss, idx);
        let conf = (WEIGHT_LUT[mask as usize] / 3).min(CONF_BUCKETS as u32 - 1) as usize;
        let weight_row = palette_bits * CONF_BUCKETS * 8 + conf * 8 + cluster as usize;
        let mixed = mix_stretched(
            unsafe { self.weights.get_unchecked(weight_row) },
            &stretched,
        );
        encoder.encode(mixed, bit);
        adapt_weights_stretched(
            unsafe { self.weights.get_unchecked_mut(weight_row) },
            &stretched,
            bit,
            mixed,
        );
        self.run = if bit != 0 { (self.run + 1).min(255) } else { 0 };
        unsafe { *miss.get_unchecked_mut(idx) = (bit == 0) as u8 };
        HeadLite {
            primary_value,
            mask,
            hash,
            bit,
        }
    }

    #[inline]
    pub(super) fn decode_head(
        &mut self,
        decoder: &mut Decoder,
        section: SectionMetadata,
        state: &mut HeadState,
        miss: &mut [u8],
        idx: usize,
    ) -> HeadLite {
        let palette_bits = section.palette_bits;
        let section_y = section.section_y;
        let memo = &mut state.memo;
        let (primary_value, mask, hash);
        let mut idx_ctx;
        if memo.matches(&state.neighbors) {
            primary_value = memo.primary_value;
            mask = memo.mask;
            hash = memo.hash;
            idx_ctx = memo.idx;
            idx_ctx[3] = (mask << 8) | self.run.min(255);
        } else {
            let (pv, m) = select_primary_value(&state.neighbors);
            let ctx = primary_contexts(
                &state.neighbors,
                pv as u32,
                m,
                self.run,
                section_y,
                palette_bits,
            );
            primary_value = pv;
            mask = m;
            hash = ctx.hash;
            idx_ctx = ctx.idx;
            memo.key = state.neighbors;
            memo.valid = true;
            memo.primary_value = pv;
            memo.mask = m;
            memo.hash = ctx.hash;
            memo.idx = ctx.idx;
        }
        let ctx = idx_ctx;
        let mut probs = [0u32; MIX_INPUTS];
        for k in 0..MIX_INPUTS {
            probs[k] =
                unsafe { *self.primary.get_unchecked(k).get_unchecked(ctx[k] as usize) } as u32;
        }
        let stretched = stretch_probs(&probs);
        let cluster = cluster_of(&state.neighbors, miss, idx);
        let conf = (WEIGHT_LUT[mask as usize] / 3).min(CONF_BUCKETS as u32 - 1) as usize;
        let weight_row = palette_bits * CONF_BUCKETS * 8 + conf * 8 + cluster as usize;
        let mixed = mix_stretched(
            unsafe { self.weights.get_unchecked(weight_row) },
            &stretched,
        );
        let bit = decoder.decode(mixed);
        let target = if bit != 0 { PROB_MAX } else { 0 };
        for k in 0..MIX_INPUTS {
            let current = probs[k] as i32;
            unsafe {
                *self
                    .primary
                    .get_unchecked_mut(k)
                    .get_unchecked_mut(ctx[k] as usize) =
                    (current + ((target - current) >> HEAD_ADAPT_SHIFT)) as u16;
            }
        }
        adapt_weights_stretched(
            unsafe { self.weights.get_unchecked_mut(weight_row) },
            &stretched,
            bit,
            mixed,
        );
        self.run = if bit != 0 { (self.run + 1).min(255) } else { 0 };
        unsafe { *miss.get_unchecked_mut(idx) = (bit == 0) as u8 };
        HeadLite {
            primary_value,
            mask,
            hash,
            bit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference_directional_hash<const REVERSED: bool>(
        seed: u32,
        neighbors: &[u16; CHAIN_SLOTS],
    ) -> u32 {
        let mut hash = seed;
        for step in 0..FIRST_ORDER {
            let slot = if REVERSED {
                FIRST_ORDER - 1 - step
            } else {
                step
            };
            hash = combine(
                hash,
                (neighbors[slot] as u32).wrapping_mul(DIRECTION_MULTS[slot]),
            );
        }
        hash
    }

    fn reference_subset_hashes(neighbors: &[u16; CHAIN_SLOTS], primary_value: u32) -> [u32; 4] {
        SUBSET_CONTEXTS.map(|(seed, slots)| {
            let mut hash = seed;
            for &slot in slots {
                hash = combine(hash, neighbors[slot] as u32);
            }
            combine(hash, primary_value)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn reference_primary_contexts(
        neighbors: &[u16; CHAIN_SLOTS],
        primary_value: u32,
        mask: u32,
        run: u32,
        section_y: usize,
        palette_bits: usize,
    ) -> PrimaryCtx {
        let neighborhood = combine(
            reference_directional_hash::<false>(FORWARD_NEIGHBORHOOD_SEED, neighbors),
            primary_value,
        );
        let rotated_neighborhood = combine(
            reference_directional_hash::<true>(REVERSE_NEIGHBORHOOD_SEED, neighbors),
            primary_value,
        );
        let [distance_two, diagonal, axial, lower_pair] =
            reference_subset_hashes(neighbors, primary_value);
        let value_scaled_hash = combine(neighborhood, primary_value.wrapping_mul(3));
        PrimaryCtx {
            idx: [
                neighborhood & PRIMARY_TABLE_MASK,
                rotated_neighborhood & PRIMARY_TABLE_MASK,
                distance_two & PRIMARY_TABLE_MASK,
                (mask << 8) | run.min(255),
                combine(primary_value, mask << 1) & PRIMARY_TABLE_MASK,
                value_scaled_hash & PRIMARY_TABLE_MASK,
                ((section_y as u32) << 5) | palette_bits as u32,
                diagonal & PRIMARY_TABLE_MASK,
                axial & PRIMARY_TABLE_MASK,
                lower_pair & PRIMARY_TABLE_MASK,
            ],
            hash: neighborhood,
        }
    }

    #[test]
    fn context_hashes_match_reference() {
        let mut state = 0x1234_5678u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut neighbors = [NONE; CHAIN_SLOTS];
        for _ in 0..100_000 {
            for slot in neighbors.iter_mut() {
                let pick = next() % 10;
                *slot = match pick {
                    0 => NONE,
                    1..=2 => (next() % 9) as u16,
                    _ => (next() % 60_001) as u16,
                };
            }
            let primary_value = if next() % 8 == 0 {
                NONE as u32
            } else {
                (next() % 60_001) as u32
            };
            let mask = next() as u32;
            let run = (next() % 256) as u32;
            let section_y = (next() % 1024) as usize;
            let palette_bits = (next() % 17) as usize;

            let new_ctx = primary_contexts(
                &neighbors,
                primary_value,
                mask,
                run,
                section_y,
                palette_bits,
            );
            let old_ctx = reference_primary_contexts(
                &neighbors,
                primary_value,
                mask,
                run,
                section_y,
                palette_bits,
            );
            assert_eq!(new_ctx.idx, old_ctx.idx, "idx mismatch");
            assert_eq!(new_ctx.hash, old_ctx.hash, "hash mismatch");
        }
    }
}
