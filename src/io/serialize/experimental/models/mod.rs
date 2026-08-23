pub(crate) mod context;
mod error;
mod predictor;
mod range;
pub(crate) mod spatial;

#[doc(hidden)]
pub mod mixer {
    pub use super::predictor::{
        ADAPT_RATE_SHIFT, CONF_BUCKETS, HEAD_ADAPT_SHIFT, MAX_BIT_DEPTH, MIX_INPUTS,
        PRIMARY_MIXER_SEED_WEIGHT, PROB_HALF, PROB_MAX, TREE_INPUTS, TREE_MIXER_SEED_WEIGHT,
        adapt_weights, adapt_weights_stretched, mix_logits, mix_stretched, stretch_probs,
    };
}
