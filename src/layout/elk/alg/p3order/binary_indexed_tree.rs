//! Port of
//! `org.eclipse.elk.alg.layered.p3order.counting.BinaryIndexedTree`.
//!
//! A Fenwick tree over `[0, max_num)`: a multiset of small integers with
//! O(log n) `add`, `remove_all`, and `rank` (count of stored values
//! strictly below an index). The crossing counter uses it to count, as
//! each edge endpoint is swept in, how many already-placed endpoints lie
//! above it.

pub struct BinaryIndexedTree {
    binary_sums: Vec<i32>,
    nums_per_index: Vec<i32>,
    size: i32,
    max_num: usize,
}

impl BinaryIndexedTree {
    pub fn new(max_num: usize) -> Self {
        Self {
            binary_sums: vec![0; max_num + 1],
            nums_per_index: vec![0; max_num],
            size: 0,
            max_num,
        }
    }

    /// Increment the count at `index`.
    pub fn add(&mut self, index: usize) {
        self.size += 1;
        self.nums_per_index[index] += 1;
        let mut i = index + 1;
        while i < self.binary_sums.len() {
            self.binary_sums[i] += 1;
            i += i & i.wrapping_neg();
        }
    }

    /// Sum of all entries strictly before `index`.
    pub fn rank(&self, index: usize) -> i32 {
        let mut i = index;
        let mut sum = 0;
        while i > 0 {
            sum += self.binary_sums[i];
            i -= i & i.wrapping_neg();
        }
        sum
    }

    pub fn size(&self) -> i32 {
        self.size
    }

    /// Remove every entry stored at `index`.
    pub fn remove_all(&mut self, index: usize) {
        let num_entries = self.nums_per_index[index];
        if num_entries == 0 {
            return;
        }
        self.nums_per_index[index] = 0;
        self.size -= num_entries;
        let mut i = index + 1;
        while i < self.binary_sums.len() {
            self.binary_sums[i] -= num_entries;
            i += i & i.wrapping_neg();
        }
    }

    pub fn clear(&mut self) {
        self.binary_sums = vec![0; self.max_num + 1];
        self.nums_per_index = vec![0; self.max_num];
        self.size = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The doc example from CrossingsCounter: sweeping endpoints top-down,
    // `rank(endPosition)` before each `add` counts the crossings.
    //   0--       add 2        rank(2)=0
    //   1-+-|     add 3        rank(3)=1  (2 is above 3)
    //     | |
    //   2-- |     remove 2, then the edge to 2 already added
    //   3----
    #[test]
    fn rank_counts_values_below_index() {
        let mut t = BinaryIndexedTree::new(4);
        t.add(2);
        assert_eq!(t.rank(2), 0); // nothing strictly below index 2
        t.add(3);
        assert_eq!(t.rank(3), 1); // the 2 is below 3 → one crossing
        assert_eq!(t.rank(4), 2); // both 2 and 3 below index 4
        assert_eq!(t.size(), 2);
        t.remove_all(2);
        assert_eq!(t.rank(4), 1);
        assert_eq!(t.size(), 1);
        assert!(!t.is_empty());
        t.remove_all(3);
        assert!(t.is_empty());
    }

    #[test]
    fn duplicates_at_an_index() {
        let mut t = BinaryIndexedTree::new(3);
        t.add(1);
        t.add(1);
        t.add(2);
        assert_eq!(t.rank(2), 2); // two entries at index 1 are below 2
        t.remove_all(1);
        assert_eq!(t.rank(3), 1); // only the entry at 2 remains
        assert_eq!(t.size(), 1);
    }
}
