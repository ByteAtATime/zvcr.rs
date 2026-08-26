pub fn find_nearest_timestamp<T, F>(source: &[T], map_fn: F, compare_to_timestamp: i64) -> i64
where
    F: Fn(&T) -> i64,
{
    if source.is_empty() {
        return compare_to_timestamp;
    }
    let mut closest = compare_to_timestamp;
    let mut min_distance = i64::MAX;

    for candidate in source {
        let candidate_timestamp = map_fn(candidate);
        let distance = (candidate_timestamp - compare_to_timestamp).abs();
        if distance < min_distance {
            min_distance = distance;
            closest = candidate_timestamp;
        }
    }
    closest
}
