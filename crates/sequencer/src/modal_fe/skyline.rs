//! Envelope (skyline) storage and Cholesky for the shifted stiffness, plus
//! the reverse Cuthill–McKee node order that keeps the envelope narrow.
//!
//! Row i stores its lower-triangle entries from its first nonzero column
//! `first[i]` through the diagonal. Cholesky fill stays inside that envelope,
//! so the factor overwrites the matrix in place; the inner loop is a dot
//! product of two contiguous row segments.

/// Symmetric matrix in row envelope storage (lower triangle).
#[derive(Clone)]
pub(crate) struct Envelope {
    first: Vec<usize>,
    start: Vec<usize>,
    data: Vec<f64>,
}

/// The entries an [`Envelope`] with rows starting at `first` stores.
pub(crate) fn envelope_size(first: &[usize]) -> usize {
    first.iter().enumerate().map(|(i, &f)| i - f + 1).sum()
}

impl Envelope {
    /// `first[i] <= i` is row i's first stored column.
    pub fn new(first: Vec<usize>) -> Self {
        let mut start = Vec::with_capacity(first.len());
        let mut size = 0;
        for (i, &f) in first.iter().enumerate() {
            debug_assert!(f <= i);
            start.push(size);
            size += i - f + 1;
        }
        Envelope { first, start, data: vec![0.0; size] }
    }

    pub fn len(&self) -> usize {
        self.first.len()
    }

    /// Stored entries (the envelope size).
    pub fn stored(&self) -> usize {
        self.data.len()
    }

    /// Add to entry (i, j); the pair is symmetric, so order does not matter.
    pub fn add(&mut self, i: usize, j: usize, value: f64) {
        let (i, j) = if j > i { (j, i) } else { (i, j) };
        debug_assert!(j >= self.first[i], "entry outside the envelope");
        self.data[self.start[i] + j - self.first[i]] += value;
    }

    pub fn diagonal(&self, i: usize) -> f64 {
        self.data[self.start[i] + i - self.first[i]]
    }

    /// y = A x for the unfactored matrix.
    #[cfg(test)]
    pub fn mul(&self, x: &[f64]) -> Vec<f64> {
        let mut y = vec![0.0; self.len()];
        for i in 0..self.len() {
            let row = &self.data[self.start[i]..self.start[i] + i - self.first[i] + 1];
            for (offset, &a) in row.iter().enumerate() {
                let j = self.first[i] + offset;
                y[i] += a * x[j];
                if j != i {
                    y[j] += a * x[i];
                }
            }
        }
        y
    }

    /// In-place L Lᵀ factorization. `progress` hears how far through it is
    /// (by work: row i costs about its length squared) before each row, and
    /// stops it with [`super::CANCELLED`] when it answers false: a row is
    /// thousands of multiply-adds, so asking costs nothing and a cancel takes
    /// effect at once even on a factorization of minutes.
    pub fn factor(&mut self, progress: &dyn Fn(f64) -> bool) -> Result<(), String> {
        let work = |i: usize| ((i - self.first[i] + 1) as f64).powi(2);
        let total: f64 = (0..self.len()).map(work).sum::<f64>().max(1.0);
        let mut done = 0.0;
        for i in 0..self.len() {
            if !progress(done / total) {
                return Err(super::CANCELLED.to_string());
            }
            done += work(i);
            let fi = self.first[i];
            let si = self.start[i];
            let (head, tail) = self.data.split_at_mut(si);
            let row_i = &mut tail[..i - fi + 1];
            for j in fi..i {
                let fj = self.first[j];
                let sj = self.start[j];
                let k0 = fi.max(fj);
                let s = dot(&row_i[k0 - fi..j - fi], &head[sj + k0 - fj..sj + j - fj]);
                let ljj = head[sj + j - fj];
                row_i[j - fi] = (row_i[j - fi] - s) / ljj;
            }
            let pivot = row_i[i - fi] - dot(&row_i[..i - fi], &row_i[..i - fi]);
            if !(pivot > 0.0) {
                return Err(format!("stiffness is not positive definite at DOF {i}"));
            }
            row_i[i - fi] = pivot.sqrt();
        }
        Ok(())
    }

    /// Overwrite b with A⁻¹ b using the factor.
    pub fn solve(&self, b: &mut [f64]) {
        let n = self.len();
        for i in 0..n {
            let fi = self.first[i];
            let row = &self.data[self.start[i]..self.start[i] + i - fi + 1];
            let s = dot(&row[..i - fi], &b[fi..i]);
            b[i] = (b[i] - s) / row[i - fi];
        }
        for i in (0..n).rev() {
            let fi = self.first[i];
            let row = &self.data[self.start[i]..self.start[i] + i - fi + 1];
            b[i] /= row[i - fi];
            let x = b[i];
            for (bk, &l) in b[fi..i].iter_mut().zip(&row[..i - fi]) {
                *bk -= l * x;
            }
        }
    }
}

/// Dot product with four independent accumulators (lets the compiler keep
/// the loop vectorized without reassociating a single sum).
pub(crate) fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut acc = [0.0; 4];
    let chunks = a.len() / 4;
    for c in 0..chunks {
        for l in 0..4 {
            acc[l] += a[4 * c + l] * b[4 * c + l];
        }
    }
    let mut s = (acc[0] + acc[1]) + (acc[2] + acc[3]);
    for i in 4 * chunks..a.len() {
        s += a[i] * b[i];
    }
    s
}

/// Reverse Cuthill–McKee order of a graph given as adjacency lists; each
/// connected piece starts from a pseudo-peripheral vertex.
pub(crate) fn reverse_cuthill_mckee(adjacency: &[Vec<u32>]) -> Vec<u32> {
    let n = adjacency.len();
    let degree = |v: usize| adjacency[v].len();
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut level = vec![u32::MAX; n];
    for seed in 0..n {
        if placed[seed] {
            continue;
        }
        // Pseudo-peripheral start: repeat BFS from the farthest,
        // lowest-degree vertex while the eccentricity grows.
        let mut start = seed;
        let mut eccentricity = 0;
        for _ in 0..8 {
            let (far, depth, visited) = bfs_far(adjacency, start, &mut level, &placed);
            for v in visited {
                level[v] = u32::MAX;
            }
            if depth <= eccentricity && eccentricity > 0 {
                break;
            }
            eccentricity = depth;
            if far == start {
                break;
            }
            start = far;
        }
        let begin = order.len();
        placed[start] = true;
        order.push(start as u32);
        let mut head = begin;
        let mut neighbors = Vec::new();
        while head < order.len() {
            let v = order[head] as usize;
            head += 1;
            neighbors.clear();
            neighbors.extend(adjacency[v].iter().map(|&w| w as usize).filter(|&w| !placed[w]));
            neighbors.sort_by_key(|&w| (degree(w), w));
            for &w in &neighbors {
                placed[w] = true;
                order.push(w as u32);
            }
        }
    }
    order.reverse();
    order
}

/// Farthest vertex (lowest degree among the last level), its depth, and the
/// visited set.
fn bfs_far(
    adjacency: &[Vec<u32>],
    start: usize,
    level: &mut [u32],
    placed: &[bool],
) -> (usize, u32, Vec<usize>) {
    let mut queue = vec![start];
    level[start] = 0;
    let mut head = 0;
    while head < queue.len() {
        let v = queue[head];
        head += 1;
        for &w in &adjacency[v] {
            let w = w as usize;
            if level[w] == u32::MAX && !placed[w] {
                level[w] = level[v] + 1;
                queue.push(w);
            }
        }
    }
    let depth = level[*queue.last().unwrap()];
    let far = queue
        .iter()
        .copied()
        .filter(|&v| level[v] == depth)
        .min_by_key(|&v| adjacency[v].len())
        .unwrap();
    (far, depth, queue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_cholesky_solves_a_banded_spd_system() {
        // Tridiagonal-plus-one-long-coupling SPD matrix.
        let n: usize = 12;
        let mut first: Vec<usize> = (0..n).map(|i| i.saturating_sub(1)).collect();
        first[9] = 2;
        let mut a = Envelope::new(first);
        for i in 0..n {
            a.add(i, i, 4.0 + i as f64 * 0.1);
            if i > 0 {
                a.add(i, i - 1, -1.0);
            }
        }
        a.add(9, 2, 0.5);
        let x_true: Vec<f64> = (0..n).map(|i| (i as f64 * 0.7).sin()).collect();
        let mut b = a.mul(&x_true);
        a.factor(&|_| true).unwrap();
        a.solve(&mut b);
        for i in 0..n {
            assert!((b[i] - x_true[i]).abs() < 1e-12);
        }
    }

    #[test]
    fn rcm_orders_a_path_end_to_end() {
        // A path 0-5-3-1-4-2: RCM must walk it, so neighbors are adjacent.
        let path = [0u32, 5, 3, 1, 4, 2];
        let mut adjacency = vec![Vec::new(); 6];
        for w in path.windows(2) {
            adjacency[w[0] as usize].push(w[1]);
            adjacency[w[1] as usize].push(w[0]);
        }
        let order = reverse_cuthill_mckee(&adjacency);
        let mut rank = [0usize; 6];
        for (r, &v) in order.iter().enumerate() {
            rank[v as usize] = r;
        }
        for w in path.windows(2) {
            assert_eq!(rank[w[0] as usize].abs_diff(rank[w[1] as usize]), 1);
        }
    }
}
