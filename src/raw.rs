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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dimension::DimensionType;
    use crate::io::file_location::RegionLocation;
    use crate::io::serialize::reader::read_file_at;
    use crate::region::delta_sequence::DeltaSequence;

    #[test]
    fn reconstruct_history_matches_snapshot_before() {
        let dir = std::path::Path::new("test_files");
        let location = RegionLocation {
            rx: -1,
            rz: -1,
            dimension_type: DimensionType::Overworld,
        };
        let file = read_file_at(dir, &location, 0).unwrap();
        let segment = file.region.segments.iter().flatten().next().unwrap();

        for section in segment.block_sections.active() {
            let history = reconstruct_history(section);
            for (i, snapshot) in history.iter().enumerate() {
                let expected = section
                    .snapshot_before(section.reverse_deltas[i].timestamp)
                    .unwrap();
                assert_eq!(snapshot.data, expected);
            }
        }

        for section in segment.biome_sections.active() {
            let history = reconstruct_history(section);
            for (i, snapshot) in history.iter().enumerate() {
                let expected = section
                    .snapshot_before(section.reverse_deltas[i].timestamp)
                    .unwrap();
                assert_eq!(snapshot.data, expected);
            }
        }
    }
}
