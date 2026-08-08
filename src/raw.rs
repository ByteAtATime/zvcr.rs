use crate::definitions::STATE_UNCHANGED;
use crate::region::delta::PackedDeltaData;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot<T> {
    pub timestamp: i64,
    pub data: T,
}

pub type SectionHistory<T> = Vec<Snapshot<T>>;

pub fn reconstruct_history<const N: usize>(
    packed: &PackedDeltaData<N>,
) -> SectionHistory<[u16; N]> {
    let deltas = &packed.reverse_deltas;
    if deltas.is_empty() {
        return Vec::new();
    }
    let mut current = deltas[0].data.unpack();
    let mut history = Vec::with_capacity(deltas.len());
    history.push(Snapshot {
        timestamp: deltas[0].timestamp,
        data: current,
    });
    for delta in deltas.iter().skip(1) {
        let unpacked = delta.data.unpack();
        for j in 0..N {
            if unpacked[j] != STATE_UNCHANGED {
                current[j] = unpacked[j];
            }
        }
        history.push(Snapshot {
            timestamp: delta.timestamp,
            data: current,
        });
    }
    history
}