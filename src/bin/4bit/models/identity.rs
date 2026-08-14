use std::sync::atomic::{AtomicU64, Ordering};

use crate::models::ModelerEntry;

use super::Modeler;

static TOTAL_SECTIONS: AtomicU64 = AtomicU64::new(0);

pub struct IdentityModeler;

impl Modeler for IdentityModeler {
    fn transform(&mut self, indices: &[u8]) -> Vec<u8> {
        TOTAL_SECTIONS.fetch_add(1, Ordering::Relaxed);

        indices.to_vec()
    }

    fn inverse(&mut self, transformed: &[u8]) -> Vec<u8> {
        transformed.to_vec()
    }

    fn print_summary(&self) {
        println!("Total sections: {}", TOTAL_SECTIONS.load(Ordering::Relaxed));
    }
}

inventory::submit! {
    ModelerEntry {
        name: "identity",
        make: || Box::new(IdentityModeler),
    }
}
