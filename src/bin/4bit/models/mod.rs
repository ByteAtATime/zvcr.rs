pub trait Modeler: Sync {
    fn transform(&mut self, indices: &[u8]) -> Vec<u8>;

    fn inverse(&mut self, transformed: &[u8]) -> Vec<u8>;

    fn transformed_len(&self, input_len: usize) -> usize;
}

pub type ModelerFactory = dyn Fn() -> Box<dyn Modeler> + Sync;

mod identity;

pub use identity::IdentityModeler;
