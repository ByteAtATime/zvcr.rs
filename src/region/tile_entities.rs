use crate::definitions::DeltaInsertionStatus;
use crate::time_utils::find_nearest_timestamp;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TileEntityPosition {
    pub x: u8,
    pub z: u8,
    pub y: u16,
}

impl TileEntityPosition {
    pub fn packed(&self) -> u32 {
        ((self.y as u32) << 16) | ((self.z as u32) << 8) | (self.x as u32)
    }

    pub fn unpack(packed: u32) -> Self {
        Self {
            x: (packed & 0xFF) as u8,
            z: ((packed >> 8) & 0xFF) as u8,
            y: (packed >> 16) as u16,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileEntity {
    pub tile_type: u32,
    pub pos: TileEntityPosition,
    pub nbt: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TileEntityDelta {
    Put(TileEntity),
    Erase,
}

pub type TileEntityDeltaMap = HashMap<TileEntityPosition, TileEntityDelta>;
pub type TileEntityList = HashMap<TileEntityPosition, TileEntity>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TileEntityListDelta {
    pub timestamp: i64,
    pub deltas: TileEntityDeltaMap,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeltaTileEntityData {
    pub reverse_deltas: Vec<TileEntityListDelta>,
}

impl DeltaTileEntityData {
    pub fn latest_snapshot(&self) -> Option<&TileEntityListDelta> {
        self.delta(0)
    }

    pub fn delta(&self, delta_index: usize) -> Option<&TileEntityListDelta> {
        self.reverse_deltas.get(delta_index)
    }

    pub fn snapshot_before(&self, timestamp: i64) -> Option<TileEntityList> {
        let latest = self.latest_snapshot()?;
        let mut snapshot = TileEntityList::new();

        for (pos, delta) in &latest.deltas {
            if let TileEntityDelta::Put(te) = delta {
                snapshot.insert(*pos, te.clone());
            }
        }

        if timestamp >= latest.timestamp {
            return Some(snapshot);
        }

        for list_delta in self.reverse_deltas.iter().skip(1) {
            for (pos, delta) in &list_delta.deltas {
                match delta {
                    TileEntityDelta::Put(te) => {
                        snapshot.insert(*pos, te.clone());
                    }
                    TileEntityDelta::Erase => {
                        snapshot.remove(pos);
                    }
                }
            }
            if timestamp >= list_delta.timestamp {
                break;
            }
        }

        Some(snapshot)
    }

    pub fn snapshot_from(&self, timestamp: i64) -> Option<TileEntityList> {
        let nearest = find_nearest_timestamp(&self.reverse_deltas, |s| s.timestamp, timestamp);
        self.snapshot_before(nearest)
    }

    pub fn insert_snapshot(
        &mut self,
        timestamp: i64,
        snapshot_slice: &[TileEntity],
    ) -> Result<usize, DeltaInsertionStatus> {
        if let Some(latest) = self.latest_snapshot() {
            if timestamp <= latest.timestamp {
                return Err(DeltaInsertionStatus::SnapshotOlderThanLatest);
            }

            let mut new_latest = TileEntityListDelta {
                timestamp,
                deltas: HashMap::with_capacity(snapshot_slice.len()),
            };

            for te in snapshot_slice {
                new_latest
                    .deltas
                    .insert(te.pos, TileEntityDelta::Put(te.clone()));
            }

            let mut deltas = TileEntityListDelta {
                timestamp: latest.timestamp,
                deltas: HashMap::new(),
            };

            for te in snapshot_slice {
                match latest.deltas.get(&te.pos) {
                    None => {
                        deltas.deltas.insert(te.pos, TileEntityDelta::Erase);
                    }
                    Some(TileEntityDelta::Put(old_te)) if old_te != te => {
                        deltas
                            .deltas
                            .insert(te.pos, TileEntityDelta::Put(old_te.clone()));
                    }
                    _ => {}
                }
            }

            for (pos, delta) in &latest.deltas {
                if !new_latest.deltas.contains_key(pos)
                    && let TileEntityDelta::Put(_) = delta {
                        deltas.deltas.insert(*pos, delta.clone());
                    }
            }

            if deltas.deltas.is_empty() {
                return Err(DeltaInsertionStatus::NoChangesMade);
            }

            let delta_count = deltas.deltas.len();
            self.reverse_deltas.remove(0);
            self.reverse_deltas.insert(0, deltas);
            self.reverse_deltas.insert(0, new_latest);

            Ok(delta_count)
        } else {
            let mut deltas = HashMap::with_capacity(snapshot_slice.len());
            for te in snapshot_slice {
                deltas.insert(te.pos, TileEntityDelta::Put(te.clone()));
            }
            let count = deltas.len();
            self.reverse_deltas
                .push(TileEntityListDelta { timestamp, deltas });
            Ok(count)
        }
    }
}
