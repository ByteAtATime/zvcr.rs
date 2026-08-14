use super::Modeler;

pub struct IdentityModeler;

impl Modeler for IdentityModeler {
    fn transform(&mut self, indices: &[u8]) -> Vec<u8> {
        indices.to_vec()
    }

    fn inverse(&mut self, transformed: &[u8]) -> Vec<u8> {
        transformed.to_vec()
    }

    fn transformed_len(&self, input_len: usize) -> usize {
        input_len
    }
}