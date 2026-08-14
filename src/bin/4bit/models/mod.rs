pub trait Modeler: Sync {
    fn transform(&mut self, indices: &[u8]) -> Vec<u8>;

    fn inverse(&mut self, transformed: &[u8]) -> Vec<u8>;

    fn print_summary(&self) {}
}

pub struct ModelerEntry {
    pub name: &'static str,
    pub make: fn() -> Box<dyn Modeler>,
}

inventory::collect!(ModelerEntry);

pub fn modeler_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = inventory::iter::<ModelerEntry>()
        .map(|entry| entry.name)
        .collect();
    names.sort_unstable();
    names
}

pub fn find_modeler(name: &str) -> &'static (dyn Fn() -> Box<dyn Modeler> + Sync) {
    let entry = inventory::iter::<ModelerEntry>()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("unknown modeler {name}, available: {}", modeler_names().join(", ")));
    &entry.make
}

mod identity;