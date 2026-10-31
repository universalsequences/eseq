//! Tensioned networks: point masses joined by strands under tension, each
//! node moving across the network's plane (a spider web, a drum skin held
//! in one, a net, a trampoline). For small swings a strand of tension T and
//! length L pulls its two ends together across the plane with stiffness
//! T / L, so the network is K φ = λ M φ with K a weighted graph Laplacian
//! (anchored nodes removed) and M the lumped node masses. A finite network
//! of a few hundred nodes is solved dense ([`super::eigen`]'s tred2/tql2 on
//! M^-1/2 K M^-1/2), once, when the instrument is generated; at run time it
//! is a modal object like any other.

use super::eigen::symmetric_eigen;

/// A network: node masses (kg), strands `(i, j, stiffness)` with stiffness
/// tension over length (N/m), and which nodes are anchored.
pub struct Net {
    pub masses: Vec<f64>,
    pub edges: Vec<(usize, usize, f64)>,
    pub fixed: Vec<bool>,
}

/// A network's lowest modes: frequencies (Hz, rising) and shapes, node-major
/// (`shapes[node * freqs.len() + mode]`; anchored nodes are 0), each scaled
/// to eseq.modal's unit modal mass: mass-weighted mean square 1/2 over the
/// nodes.
#[derive(Debug)]
pub struct NetModes {
    pub freqs: Vec<f64>,
    pub shapes: Vec<f64>,
}

/// The most nodes a network may have (a dense solve is O(n^3)).
pub const MAX_NODES: usize = 1500;

/// Solve `net` for its lowest `count` modes (fewer when it has fewer free
/// nodes). Modes with no stiffness (a piece anchored nowhere moving as a
/// whole) are left out.
pub fn net_modes(net: &Net, count: usize) -> Result<NetModes, String> {
    let n = net.masses.len();
    if n == 0 {
        return Err("a net needs nodes".to_string());
    }
    if n > MAX_NODES {
        return Err(format!("a net may have at most {MAX_NODES} nodes, not {n}"));
    }
    if net.fixed.len() != n {
        return Err("a net's anchors must name one flag per node".to_string());
    }
    if let Some(m) = net.masses.iter().find(|m| !(m.is_finite() && **m > 0.0)) {
        return Err(format!("a net's node masses must be positive, not {m}"));
    }
    // free nodes, renumbered
    let mut index = vec![usize::MAX; n];
    let mut free = Vec::new();
    for node in 0..n {
        if !net.fixed[node] {
            index[node] = free.len();
            free.push(node);
        }
    }
    let f = free.len();
    if f == 0 {
        return Err("every node of the net is anchored".to_string());
    }
    let mut k = vec![0.0; f * f];
    for &(i, j, stiffness) in &net.edges {
        if i >= n || j >= n {
            return Err(format!("a net's strand ({i} {j}) names a node it does not have ({n} nodes)"));
        }
        if !(stiffness.is_finite() && stiffness >= 0.0) {
            return Err(format!("a net's strand ({i} {j}) has stiffness {stiffness}"));
        }
        if i == j {
            continue;
        }
        let (a, b) = (index[i], index[j]);
        if a != usize::MAX {
            k[a * f + a] += stiffness;
        }
        if b != usize::MAX {
            k[b * f + b] += stiffness;
        }
        if a != usize::MAX && b != usize::MAX {
            k[a * f + b] -= stiffness;
            k[b * f + a] -= stiffness;
        }
    }
    let root: Vec<f64> = free.iter().map(|&node| net.masses[node].sqrt()).collect();
    for a in 0..f {
        for b in 0..f {
            k[a * f + b] /= root[a] * root[b];
        }
    }
    let (values, vectors) = symmetric_eigen(&k, f);
    let top = values.iter().copied().fold(0.0_f64, f64::max);
    let total_mass: f64 = net.masses.iter().sum();
    let scale = (total_mass / 2.0).sqrt();
    let keep: Vec<usize> = (0..f).filter(|&m| values[m] > top * 1e-10 && values[m] > 0.0).take(count).collect();
    let modes = keep.len();
    let mut shapes = vec![0.0; n * modes];
    for (slot, &m) in keep.iter().enumerate() {
        // v is unit length, so phi = M^-1/2 v has phi' M phi = 1
        for (a, &node) in free.iter().enumerate() {
            shapes[node * modes + slot] = vectors[a * f + m] / root[a] * scale;
        }
    }
    let freqs = keep.iter().map(|&m| values[m].sqrt() / std::f64::consts::TAU).collect();
    Ok(NetModes { freqs, shapes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A string of n interior beads, anchored at both ends: the textbook
    /// beaded string, w_k = 2 sqrt(s / m) sin(k pi / (2 (n + 1))).
    #[test]
    fn a_beaded_string_rings_at_its_closed_form_frequencies() {
        let beads = 9;
        let (m, s) = (0.01, 40.0);
        let n = beads + 2;
        let net = Net {
            masses: vec![m; n],
            edges: (0..n - 1).map(|i| (i, i + 1, s)).collect(),
            fixed: (0..n).map(|i| i == 0 || i == n - 1).collect(),
        };
        let modes = net_modes(&net, 4).unwrap();
        assert_eq!(modes.freqs.len(), 4);
        for (k, f) in modes.freqs.iter().enumerate() {
            let w = 2.0 * (s / m).sqrt() * ((k + 1) as f64 * std::f64::consts::PI / (2.0 * (beads + 1) as f64)).sin();
            assert!((f - w / std::f64::consts::TAU).abs() < 1e-9 * f, "mode {k}: {f} vs {}", w / std::f64::consts::TAU);
        }
        // anchors do not move; the lowest mode is one arch (no sign change);
        // mass-weighted mean square is 1/2
        assert_eq!(modes.shapes[0], 0.0);
        assert_eq!(modes.shapes[(n - 1) * 4], 0.0);
        let first: Vec<f64> = (1..n - 1).map(|i| modes.shapes[i * 4]).collect();
        assert!(first.iter().all(|v| v * first[0] > 0.0), "{first:?}");
        let mean_square = (0..n).map(|i| m * modes.shapes[i * 4].powi(2)).sum::<f64>() / (m * n as f64);
        assert!((mean_square - 0.5).abs() < 1e-9, "{mean_square}");
    }

    #[test]
    fn a_free_piece_moving_as_a_whole_is_left_out() {
        // two beads on one strand, anchored nowhere: one rigid mode (left
        // out) and one at sqrt(2 s / m)
        let net = Net { masses: vec![1.0, 1.0], edges: vec![(0, 1, 2.0)], fixed: vec![false, false] };
        let modes = net_modes(&net, 4).unwrap();
        assert_eq!(modes.freqs.len(), 1);
        assert!((modes.freqs[0] - 2.0 / std::f64::consts::TAU).abs() < 1e-12);
    }

    #[test]
    fn bad_nets_say_what_is_wrong() {
        let anchored = Net { masses: vec![1.0], edges: vec![], fixed: vec![true] };
        assert!(net_modes(&anchored, 2).unwrap_err().contains("anchored"));
        let dangling = Net { masses: vec![1.0], edges: vec![(0, 3, 1.0)], fixed: vec![false] };
        assert!(net_modes(&dangling, 2).unwrap_err().contains("does not have"));
        let massless = Net { masses: vec![0.0], edges: vec![], fixed: vec![false] };
        assert!(net_modes(&massless, 2).unwrap_err().contains("positive"));
    }
}
