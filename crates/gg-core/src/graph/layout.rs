//! Lane-based commit graph layout. Commits arrive newest first; each gets a lane (column)
//! and colour, plus the line segments the UI draws in its row.

use serde::Serialize;

/// Which part of a row an [`Edge`] spans.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeHalf {
    /// From the row's top border at lane `from` to the node at lane `to`.
    Top,
    /// From the node at lane `from` to the row's bottom border at lane `to`.
    Bottom,
    /// Straight through the row at lane `from` (`from == to`).
    Full,
}

/// One line segment drawn in a row.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: u16,
    pub to: u16,
    pub color: u16,
    pub half: EdgeHalf,
}

/// Layout of one commit's row.
#[derive(Clone, Debug, PartialEq)]
pub struct RowLayout {
    /// Lane of the commit's node.
    pub lane: u16,
    /// Colour index of the commit's node.
    pub color: u16,
    pub edges: Vec<Edge>,
}

/// Lane indices are `u16`, so at most this many lanes exist at once.
const MAX_LANES: usize = u16::MAX as usize + 1;

/// One lane slot. A free slot keeps its `hash` buffer so the next occupant reuses it.
#[derive(Debug, Default)]
struct Lane {
    occupied: bool,
    /// Cheap prefilter for `hash` comparisons: see [`hash_key`].
    key: u64,
    /// The commit this lane's line is heading to.
    hash: String,
    color: u16,
}

impl Lane {
    fn awaits(&self, key: u64, hash: &str) -> bool {
        self.occupied && self.key == key && self.hash == hash
    }

    fn occupy(&mut self, key: u64, hash: &str, color: u16) {
        self.occupied = true;
        self.key = key;
        self.hash.clear();
        self.hash.push_str(hash);
        self.color = color;
    }
}

/// The first eight bytes of a hash, so most mismatches are found with one integer compare.
fn hash_key(hash: &str) -> u64 {
    let bytes = hash.as_bytes();
    let n = bytes.len().min(8);
    let mut buf = [0u8; 8];
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_le_bytes(buf)
}

/// Assigns lanes and colours to commits pushed newest first, and emits each row's edges.
///
/// Each lane awaits a commit (the parent its line is heading to). A commit takes the lowest
/// lane awaiting it, or a free lane if none does; its first parent continues in its lane and
/// further parents join the lane already awaiting them or open a new one. A parent that never
/// arrives leaves its lane occupied (the line dangles off the bottom).
///
/// When all 65,536 lanes are occupied the last lane is reused and the line it carried is
/// dropped. Colours wrap around after `u16::MAX`.
#[derive(Debug)]
pub struct LaneAllocator {
    /// Lane slots; only `lanes[..len]` are in use, later slots are spare buffers.
    lanes: Vec<Lane>,
    /// One past the highest occupied lane (trailing free lanes are truncated).
    len: usize,
    next_color: u16,
    max_lane: u16,
    lane_cap: usize,
}

impl Default for LaneAllocator {
    fn default() -> Self {
        Self::with_lane_cap(MAX_LANES)
    }
}

impl LaneAllocator {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_lane_cap(lane_cap: usize) -> Self {
        Self {
            lanes: Vec::new(),
            len: 0,
            next_color: 0,
            max_lane: 0,
            lane_cap,
        }
    }

    /// Lays out the next (older) commit. `parents` lists its parents, first parent first.
    pub fn push(&mut self, hash: &str, parents: &[String]) -> RowLayout {
        let mut edges = Vec::with_capacity(self.len + parents.len());
        let key = hash_key(hash);

        // Lines arriving at this commit end at the lowest such lane; the others pass through.
        let mut node: Option<(u16, u16)> = None;
        for (i, lane) in self.lanes[..self.len].iter_mut().enumerate() {
            if !lane.occupied {
                continue;
            }
            let i = i as u16; // `len <= lane_cap <= MAX_LANES`, so every index fits.
            if lane.awaits(key, hash) {
                let (node_lane, _) = *node.get_or_insert((i, lane.color));
                edges.push(edge(i, node_lane, lane.color, EdgeHalf::Top));
                if i != node_lane {
                    lane.occupied = false;
                }
            } else {
                edges.push(edge(i, i, lane.color, EdgeHalf::Full));
            }
        }
        let (lane, color) = match node {
            Some(node) => node,
            None => (self.alloc(), self.new_color()),
        };

        match parents.split_first() {
            None => self.lanes[usize::from(lane)].occupied = false,
            Some((first, rest)) => {
                self.lanes[usize::from(lane)].occupy(hash_key(first), first, color);
                edges.push(edge(lane, lane, color, EdgeHalf::Bottom));
                for (k, parent) in rest.iter().enumerate() {
                    // `parents[..=k]` are the parents before this one; repeats are malformed.
                    if parents[..=k].contains(parent) {
                        continue;
                    }
                    let parent_key = hash_key(parent);
                    let awaiting = self.lanes[..self.len]
                        .iter()
                        .position(|l| l.awaits(parent_key, parent));
                    let (to, to_color) = match awaiting {
                        Some(i) => (i as u16, self.lanes[i].color),
                        None => {
                            let (to, to_color) = (self.alloc(), self.new_color());
                            self.lanes[usize::from(to)].occupy(parent_key, parent, to_color);
                            (to, to_color)
                        }
                    };
                    edges.push(edge(lane, to, to_color, EdgeHalf::Bottom));
                }
            }
        }

        while self.len > 0 && !self.lanes[self.len - 1].occupied {
            self.len -= 1;
        }
        RowLayout { lane, color, edges }
    }

    /// Highest lane index used so far.
    pub fn max_lane(&self) -> u16 {
        self.max_lane
    }

    /// The lowest free lane, growing the lane list if none is free. The caller occupies it.
    fn alloc(&mut self) -> u16 {
        let free = self.lanes[..self.len].iter().position(|l| !l.occupied);
        let i = match free {
            Some(i) => i,
            None if self.len < self.lane_cap => {
                if self.len == self.lanes.len() {
                    self.lanes.push(Lane::default());
                }
                self.len += 1;
                self.len - 1
            }
            None => self.lane_cap - 1,
        } as u16;
        self.max_lane = self.max_lane.max(i);
        i
    }

    fn new_color(&mut self) -> u16 {
        let color = self.next_color;
        self.next_color = color.wrapping_add(1);
        color
    }
}

fn edge(from: u16, to: u16, color: u16, half: EdgeHalf) -> Edge {
    Edge {
        from,
        to,
        color,
        half,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    use super::*;

    fn top(from: u16, to: u16, color: u16) -> Edge {
        Edge {
            from,
            to,
            color,
            half: EdgeHalf::Top,
        }
    }

    fn bottom(from: u16, to: u16, color: u16) -> Edge {
        Edge {
            from,
            to,
            color,
            half: EdgeHalf::Bottom,
        }
    }

    fn full(lane: u16, color: u16) -> Edge {
        Edge {
            from: lane,
            to: lane,
            color,
            half: EdgeHalf::Full,
        }
    }

    fn push(alloc: &mut LaneAllocator, hash: &str, parents: &[&str]) -> RowLayout {
        let parents: Vec<String> = parents.iter().map(|p| p.to_string()).collect();
        alloc.push(hash, &parents)
    }

    /// Asserts lane, colour and the edge set (order-insensitive, duplicates rejected).
    fn assert_row(row: &RowLayout, lane: u16, color: u16, edges: &[Edge]) {
        assert_eq!(
            (row.lane, row.color),
            (lane, color),
            "lane/color of {row:?}"
        );
        let got: HashSet<&Edge> = row.edges.iter().collect();
        assert_eq!(got.len(), row.edges.len(), "duplicate edges in {row:?}");
        let want: HashSet<&Edge> = edges.iter().collect();
        assert_eq!(got, want, "edges of {row:?}");
    }

    #[test]
    fn graph_linear() {
        let mut a = LaneAllocator::new();
        let c3 = push(&mut a, "c3", &["c2"]);
        let c2 = push(&mut a, "c2", &["c1"]);
        let c1 = push(&mut a, "c1", &[]);
        assert_row(&c3, 0, 0, &[bottom(0, 0, 0)]);
        assert_row(&c2, 0, 0, &[top(0, 0, 0), bottom(0, 0, 0)]);
        assert_row(&c1, 0, 0, &[top(0, 0, 0)]);
        assert_eq!(a.max_lane(), 0);
    }

    fn branch_and_merge(a: &mut LaneAllocator) -> [RowLayout; 4] {
        [
            push(a, "M", &["A", "B"]),
            push(a, "B", &["C"]),
            push(a, "A", &["C"]),
            push(a, "C", &[]),
        ]
    }

    #[test]
    fn graph_branch_and_merge() {
        let mut a = LaneAllocator::new();
        let [m, b, a_row, c] = branch_and_merge(&mut a);
        assert_row(&m, 0, 0, &[bottom(0, 0, 0), bottom(0, 1, 1)]);
        assert_row(&b, 1, 1, &[full(0, 0), top(1, 1, 1), bottom(1, 1, 1)]);
        assert_row(&a_row, 0, 0, &[top(0, 0, 0), bottom(0, 0, 0), full(1, 1)]);
        assert_row(&c, 0, 0, &[top(0, 0, 0), top(1, 0, 1)]);
        assert_eq!(a.max_lane(), 1);
    }

    #[test]
    fn graph_two_tips_share_parent() {
        let mut a = LaneAllocator::new();
        let x = push(&mut a, "X", &["P"]);
        let y = push(&mut a, "Y", &["P"]);
        let p = push(&mut a, "P", &[]);
        assert_row(&x, 0, 0, &[bottom(0, 0, 0)]);
        assert_row(&y, 1, 1, &[full(0, 0), bottom(1, 1, 1)]);
        assert_row(&p, 0, 0, &[top(0, 0, 0), top(1, 0, 1)]);
    }

    #[test]
    fn graph_lane_reuse() {
        let mut a = LaneAllocator::new();
        branch_and_merge(&mut a);
        let z = push(&mut a, "Z", &[]);
        assert_row(&z, 0, 2, &[]);
        assert_eq!(a.max_lane(), 1);
    }

    /// A stash row has its base as its one parent (its index and untracked commits are left
    /// out before layout): one short lane that joins its base, like a branch of one commit,
    /// and no lanes for the helpers.
    #[test]
    fn graph_stash_row_joins_its_base() {
        let mut a = LaneAllocator::new();
        let w = push(&mut a, "W", &["B"]);
        let c = push(&mut a, "C", &["B"]);
        let b = push(&mut a, "B", &["A"]);
        let a_row = push(&mut a, "A", &[]);
        assert_row(&w, 0, 0, &[bottom(0, 0, 0)]);
        assert_row(&c, 1, 1, &[full(0, 0), bottom(1, 1, 1)]);
        assert_row(&b, 0, 0, &[top(0, 0, 0), top(1, 0, 1), bottom(0, 0, 0)]);
        assert_row(&a_row, 0, 0, &[top(0, 0, 0)]);
        assert_eq!(a.max_lane(), 1);
        check_invariants(&[w, c, b, a_row]);

        // With all three parents, the helpers would hold two more lanes open.
        let mut a = LaneAllocator::new();
        let w = push(&mut a, "W", &["B", "I", "U"]);
        assert_eq!(w.edges.len(), 3);
        assert_eq!(a.max_lane(), 2);
    }

    #[test]
    fn graph_missing_parent_dangles() {
        let mut a = LaneAllocator::new();
        let s = push(&mut a, "S", &["GONE"]);
        let t = push(&mut a, "T", &[]);
        assert_row(&s, 0, 0, &[bottom(0, 0, 0)]);
        assert_row(&t, 1, 1, &[full(0, 0)]);
    }

    #[test]
    fn graph_octopus() {
        let mut a = LaneAllocator::new();
        let o = push(&mut a, "O", &["A", "B", "C"]);
        assert_row(
            &o,
            0,
            0,
            &[bottom(0, 0, 0), bottom(0, 1, 1), bottom(0, 2, 2)],
        );
        assert_eq!(a.max_lane(), 2);
    }

    #[test]
    fn graph_second_parent_already_awaited_joins_its_lane() {
        let mut a = LaneAllocator::new();
        push(&mut a, "X", &["P"]);
        let m = push(&mut a, "M", &["Q", "P"]);
        assert_row(&m, 1, 1, &[full(0, 0), bottom(1, 1, 1), bottom(1, 0, 0)]);
    }

    #[test]
    fn graph_merge_reuses_lane_freed_on_the_same_row() {
        let mut a = LaneAllocator::new();
        push(&mut a, "X", &["M"]);
        push(&mut a, "Y", &["M"]);
        // Lane 1 is freed when M arrives, then taken by M's second parent.
        let m = push(&mut a, "M", &["A", "B"]);
        assert_row(
            &m,
            0,
            0,
            &[top(0, 0, 0), top(1, 0, 1), bottom(0, 0, 0), bottom(0, 1, 2)],
        );
    }

    #[test]
    fn graph_duplicate_parents_emit_no_duplicate_edges() {
        let mut a = LaneAllocator::new();
        let d = push(&mut a, "D", &["P", "P", "Q", "Q"]);
        assert_row(&d, 0, 0, &[bottom(0, 0, 0), bottom(0, 1, 1)]);
        let p = push(&mut a, "P", &[]);
        assert_row(&p, 0, 0, &[top(0, 0, 0), full(1, 1)]);
    }

    #[test]
    fn graph_edges_serialize_as_ui_type() {
        let json = serde_json::to_string(&[top(0, 1, 2), bottom(3, 4, 5), full(6, 7)]).unwrap();
        assert_eq!(
            json,
            r#"[{"from":0,"to":1,"color":2,"half":"top"},{"from":3,"to":4,"color":5,"half":"bottom"},{"from":6,"to":6,"color":7,"half":"full"}]"#
        );
    }

    #[test]
    fn graph_colors_wrap_instead_of_overflowing() {
        let mut a = LaneAllocator::new();
        let mut last = None;
        for _ in 0..=u16::MAX as usize {
            last = Some(push(&mut a, "root", &[]));
        }
        assert_eq!(last.unwrap().color, u16::MAX);
        assert_eq!(push(&mut a, "root", &[]).color, 0);
    }

    #[test]
    fn graph_lanes_saturate_without_panicking() {
        let mut a = LaneAllocator::with_lane_cap(2);
        assert_eq!(push(&mut a, "A", &["X"]).lane, 0);
        assert_eq!(push(&mut a, "B", &["Y"]).lane, 1);
        // Every lane is taken: the last lane is reused.
        let c = push(&mut a, "C", &["Z", "W"]);
        assert_eq!(c.lane, 1);
        assert!(c.edges.iter().all(|e| e.from <= 1 && e.to <= 1));
        assert_eq!(push(&mut a, "D", &["V"]).lane, 1);
        assert_eq!(a.max_lane(), 1);
    }

    /// Small deterministic LCG (Knuth's MMIX constants).
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }
    }

    /// Checks the drawing invariants over consecutive rows: edges attach to the node, no
    /// duplicate segments, and lines never appear or vanish between rows.
    fn check_invariants(rows: &[RowLayout]) {
        for (i, row) in rows.iter().enumerate() {
            let mut seen = HashSet::new();
            for e in &row.edges {
                assert!(
                    seen.insert((e.half, e.from, e.to)),
                    "row {i}: duplicate {e:?}"
                );
                match e.half {
                    EdgeHalf::Top => assert_eq!(e.to, row.lane, "row {i}: {e:?}"),
                    EdgeHalf::Bottom => assert_eq!(e.from, row.lane, "row {i}: {e:?}"),
                    EdgeHalf::Full => assert_eq!(e.from, e.to, "row {i}: {e:?}"),
                }
                let at_node = (e.half == EdgeHalf::Top && e.from == row.lane)
                    || (e.half == EdgeHalf::Bottom && e.to == row.lane);
                if at_node {
                    assert_eq!(e.color, row.color, "row {i}: node colour vs {e:?}");
                }
                assert!(
                    !(e.half == EdgeHalf::Full && e.from == row.lane),
                    "row {i}: line through the node {e:?}"
                );
            }
        }
        let leaves = |row: &RowLayout| -> Vec<(u16, u16)> {
            row.edges
                .iter()
                .filter(|e| e.half != EdgeHalf::Top)
                .map(|e| (e.to, e.color))
                .collect()
        };
        let enters = |row: &RowLayout| -> Vec<(u16, u16)> {
            row.edges
                .iter()
                .filter(|e| e.half != EdgeHalf::Bottom)
                .map(|e| (e.from, e.color))
                .collect()
        };
        assert!(enters(&rows[0]).is_empty(), "first row has incoming lines");
        for (i, pair) in rows.windows(2).enumerate() {
            let (out, inc) = (leaves(&pair[0]), enters(&pair[1]));
            for l in &out {
                assert!(
                    inc.contains(l),
                    "line (lane, colour) {l:?} vanishes after row {i}"
                );
            }
            for l in &inc {
                assert!(
                    out.contains(l),
                    "line {l:?} appears from nowhere at row {}",
                    i + 1
                );
            }
        }
    }

    #[test]
    fn graph_random_dag_keeps_lines_continuous() {
        const N: usize = 5_000;
        let mut rng = Lcg(0x5eed);
        // Zero-padded hashes share long prefixes, exercising the full-hash comparison.
        let hash = |i: usize| format!("{i:040x}");
        let mut a = LaneAllocator::new();
        let mut rows = Vec::with_capacity(N);
        for i in 0..N {
            let roll = rng.next() % 100;
            let count = match roll {
                _ if i == N - 1 => 0,
                0..3 => 0,
                3..78 => 1,
                78..96 => 2,
                _ => 3,
            };
            // Parents are older commits nearby; duplicates happen and must be tolerated.
            let parents: Vec<String> = (0..count)
                .map(|_| hash((i + 1 + (rng.next() % 30) as usize).min(N - 1)))
                .collect();
            let row = a.push(&hash(i), &parents);
            assert!(row.lane <= a.max_lane());
            rows.push(row);
        }
        check_invariants(&rows);
        let last = rows.last().unwrap();
        assert!(
            last.edges.iter().all(|e| e.half == EdgeHalf::Top),
            "every line ends at the last commit: {last:?}"
        );
        assert!(a.max_lane() > 2, "the DAG should branch: {}", a.max_lane());
    }

    /// A realistic 40-hex hash for synthetic commit `i`.
    fn synthetic_hash(i: usize) -> String {
        let mut z = (i as u64).wrapping_add(0x9e3779b97f4a7c15);
        let mut mix = || {
            z = z.wrapping_add(0x9e3779b97f4a7c15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
            x ^ (x >> 31)
        };
        format!("{:016x}{:016x}{:08x}", mix(), mix(), mix() as u32)
    }

    /// Linear history with a five-commit side branch merged in every 50 commits.
    fn synthetic_history(n: usize) -> Vec<(String, Vec<String>)> {
        let hashes: Vec<String> = (0..n).map(synthetic_hash).collect();
        (0..n)
            .map(|i| {
                let parents: Vec<usize> = match i % 50 {
                    0 => vec![i + 1, i + 40], // merge: main continues, side branch at +40
                    39 | 44 => vec![i - (i % 50) + 45], // main tip and side tip fork from +45
                    _ => vec![i + 1],
                };
                let parents = parents
                    .into_iter()
                    .filter(|&p| p < n)
                    .map(|p| hashes[p].clone())
                    .collect();
                (hashes[i].clone(), parents)
            })
            .collect()
    }

    #[test]
    fn graph_synthetic_history_is_well_formed() {
        let mut a = LaneAllocator::new();
        let rows: Vec<RowLayout> = synthetic_history(500)
            .iter()
            .map(|(h, p)| a.push(h, p))
            .collect();
        check_invariants(&rows);
        assert_eq!(a.max_lane(), 1);
    }

    #[test]
    #[cfg_attr(debug_assertions, ignore = "timing is only meaningful with --release")]
    fn graph_scale() {
        const N: usize = 100_000;
        let commits = synthetic_history(N);
        let mut a = LaneAllocator::new();
        let mut edges = 0usize;
        let start = Instant::now();
        for (hash, parents) in &commits {
            edges += std::hint::black_box(a.push(hash, parents)).edges.len();
        }
        let elapsed = start.elapsed();
        assert!(edges > N);
        assert_eq!(a.max_lane(), 1);
        if !cfg!(debug_assertions) {
            assert!(
                elapsed < Duration::from_millis(250),
                "{N} commits took {elapsed:?}"
            );
        }
    }
}
