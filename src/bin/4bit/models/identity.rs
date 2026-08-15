use std::sync::atomic::{AtomicU64, Ordering};

use crate::models::ModelerEntry;

use super::{Modeler, SectionContext};

static TOTAL_STREAMS: AtomicU64 = AtomicU64::new(0);

pub struct IdentityModeler;

impl Modeler for IdentityModeler {
    fn transform(&mut self, _ctx: &SectionContext, indices: &[u8]) -> Vec<u8> {
        TOTAL_STREAMS.fetch_add(1, Ordering::Relaxed);

        let mut packed = vec![0u8; indices.len().div_ceil(2)];
        for (i, &nibble) in indices.iter().enumerate() {
            packed[i / 2] |= nibble << ((i % 2) * 4);
        }
        packed
    }

    fn inverse(&mut self, _ctx: &SectionContext, transformed: &[u8]) -> Vec<u8> {
        let mut indices = Vec::with_capacity(transformed.len() * 2);
        for &byte in transformed {
            indices.push(byte & 0x0F);
            indices.push(byte >> 4);
        }
        indices
    }

    fn print_summary(&self) {
        println!("Total streams: {}", TOTAL_STREAMS.load(Ordering::Relaxed));
    }
}

inventory::submit! {
    ModelerEntry {
        name: "identity",
        make: || Box::new(IdentityModeler),
    }
}
