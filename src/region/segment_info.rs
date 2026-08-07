use crate::time_utils::find_nearest_timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum SegmentStateType {
    #[default]
    Unknown = 0,
    New = 1,
    Old = 2,
}

impl SegmentStateType {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unknown),
            1 => Some(Self::New),
            2 => Some(Self::Old),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentState {
    pub state_type: SegmentStateType,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentInfo {
    pub segment_states: Vec<SegmentState>,
}

impl SegmentInfo {
    pub fn latest_snapshot(&self) -> Option<&SegmentState> {
        self.delta(0)
    }

    pub fn delta(&self, delta_index: usize) -> Option<&SegmentState> {
        self.segment_states.get(delta_index)
    }

    pub fn snapshot_before(&self, timestamp: i64) -> Option<SegmentState> {
        let latest = self.latest_snapshot()?;
        let mut latest_state_type = latest.state_type;

        for state in &self.segment_states {
            latest_state_type = state.state_type;
            if timestamp >= state.timestamp {
                break;
            }
        }
        Some(SegmentState {
            state_type: latest_state_type,
            timestamp,
        })
    }

    pub fn snapshot_from(&self, timestamp: i64) -> Option<SegmentState> {
        let nearest = find_nearest_timestamp(&self.segment_states, |s| s.timestamp, timestamp);
        self.snapshot_before(nearest)
    }

    pub fn insert_snapshot(&mut self, new_state: SegmentState) -> bool {
        if let Some(latest) = self.latest_snapshot()
            && (new_state.timestamp <= latest.timestamp
                || latest.state_type == new_state.state_type)
        {
            return false;
        }
        self.segment_states.insert(0, new_state);
        true
    }
}
