//! The solid spec: the JSON a finite-element modal object is built from.
//!
//! Lengths are metres, density kg/m³, Young's modulus Pa. Every field the
//! solver reads has an explicit default, and serialization writes them all,
//! so the serialized spec is canonical: the same object always hashes the
//! same (see [`super::spec_hash`]).

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolidSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub solid: Solid,
    #[serde(default)]
    pub resolution: Resolution,
    pub material: Material,
    /// Nodes inside this region are clamped; a free body when absent.
    #[serde(default)]
    pub block: Option<Region>,
    /// Elastic modes wanted (rigid-body modes are never returned).
    #[serde(default = "default_modes")]
    pub modes: usize,
    #[serde(default)]
    pub mesh: MeshMode,
}

fn default_modes() -> usize {
    20
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Material {
    pub density: f64,
    pub young: f64,
    pub poisson: f64,
}

/// Target element size. `cells` divides the largest bounding-box extent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Resolution {
    Cells(usize),
    ElementSize(f64),
}

impl Default for Resolution {
    fn default() -> Self {
        Resolution::Cells(32)
    }
}

/// `auto` meshes a lone revolve / tube / shell / cylinder with a mapped
/// (body-fitted) hex mesh and everything else by voxels; `voxel` forces
/// voxels; `mapped` requires a mappable root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshMode {
    #[default]
    Auto,
    Voxel,
    Mapped,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    X,
    Y,
    #[default]
    Z,
}

/// Mapped-mesh element counts for a revolve; each defaults from the
/// resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segments {
    #[serde(default)]
    pub along: Option<usize>,
    #[serde(default)]
    pub around: Option<usize>,
    #[serde(default)]
    pub through: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Solid {
    Box {
        size: [f64; 3],
        #[serde(default)]
        center: [f64; 3],
    },
    Cylinder {
        radius: f64,
        height: f64,
        #[serde(default)]
        center: [f64; 3],
        #[serde(default)]
        axis: Axis,
    },
    /// Hollow cylinder.
    Tube {
        outer_radius: f64,
        inner_radius: f64,
        height: f64,
        #[serde(default)]
        center: [f64; 3],
        #[serde(default)]
        axis: Axis,
    },
    Sphere {
        radius: f64,
        #[serde(default)]
        center: [f64; 3],
    },
    /// Spherical shell.
    Shell {
        outer_radius: f64,
        inner_radius: f64,
        #[serde(default)]
        center: [f64; 3],
    },
    /// Solid of revolution about `axis` through `center`. `profile` points
    /// are `[r, z]` or `[r, z, wall]`. With a wall thickness (per point, or
    /// the global `thickness`) the profile is the wall's mid-surface, so a
    /// bell, bowl or cymbal is its cross-section curve; with none it is a
    /// closed (r, z) polygon revolved as a solid.
    Revolve {
        profile: Vec<Vec<f64>>,
        #[serde(default)]
        thickness: Option<f64>,
        #[serde(default)]
        center: [f64; 3],
        #[serde(default)]
        axis: Axis,
        #[serde(default)]
        segments: Segments,
    },
    Union(Vec<Solid>),
    /// The first solid minus all the others.
    Difference(Vec<Solid>),
    /// A closed triangle mesh from a Wavefront .obj (relative paths resolve
    /// against the spec's directory), scaled then offset.
    Obj {
        path: String,
        #[serde(default = "default_scale")]
        scale: f64,
        #[serde(default)]
        offset: [f64; 3],
    },
}

fn default_scale() -> f64 {
    1.0
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Region {
    Box { min: [f64; 3], max: [f64; 3] },
    Sphere { center: [f64; 3], radius: f64 },
}

impl Region {
    pub fn contains(&self, p: [f64; 3], tol: f64) -> bool {
        match *self {
            Region::Box { min, max } => (0..3).all(|i| p[i] >= min[i] - tol && p[i] <= max[i] + tol),
            Region::Sphere { center, radius } => dist(p, center) <= radius + tol,
        }
    }
}

impl SolidSpec {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let spec: SolidSpec = serde_json::from_str(text).map_err(|error| error.to_string())?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::from_json(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn validate(&self) -> Result<(), String> {
        let m = &self.material;
        if !(m.density > 0.0 && m.young > 0.0) {
            return Err("material density and young must be positive".into());
        }
        if !(m.poisson > -1.0 && m.poisson < 0.5) {
            return Err("material poisson must be in (-1, 0.5)".into());
        }
        match self.resolution {
            Resolution::Cells(0) => return Err("resolution cells must be positive".into()),
            Resolution::ElementSize(h) if !(h > 0.0) => {
                return Err("resolution element_size must be positive".into())
            }
            _ => {}
        }
        if self.modes == 0 {
            return Err("modes must be positive".into());
        }
        self.solid.validate()
    }
}

impl Solid {
    fn validate(&self) -> Result<(), String> {
        let positive = |name: &str, v: f64| {
            if v > 0.0 {
                Ok(())
            } else {
                Err(format!("{name} must be positive"))
            }
        };
        match self {
            Solid::Box { size, .. } => size.iter().try_for_each(|&s| positive("box size", s)),
            Solid::Cylinder { radius, height, .. } => {
                positive("cylinder radius", *radius)?;
                positive("cylinder height", *height)
            }
            Solid::Tube { outer_radius, inner_radius, height, .. } => {
                positive("tube height", *height)?;
                if !(*inner_radius >= 0.0 && outer_radius > inner_radius) {
                    return Err("tube needs 0 <= inner_radius < outer_radius".into());
                }
                Ok(())
            }
            Solid::Sphere { radius, .. } => positive("sphere radius", *radius),
            Solid::Shell { outer_radius, inner_radius, .. } => {
                if !(*inner_radius > 0.0 && outer_radius > inner_radius) {
                    return Err("shell needs 0 < inner_radius < outer_radius".into());
                }
                Ok(())
            }
            Solid::Revolve { profile, thickness, .. } => {
                if profile.len() < 2 {
                    return Err("revolve profile needs at least 2 points".into());
                }
                for point in profile {
                    if point.len() != 2 && point.len() != 3 {
                        return Err("revolve profile points are [r, z] or [r, z, wall]".into());
                    }
                    if point[0] < 0.0 {
                        return Err("revolve profile radii must be >= 0".into());
                    }
                    if point.len() == 3 {
                        positive("revolve wall", point[2])?;
                    }
                }
                if let Some(t) = thickness {
                    positive("revolve thickness", *t)?;
                }
                let filled = thickness.is_none() && profile.iter().all(|p| p.len() == 2);
                if filled && profile.len() < 3 {
                    return Err("a revolve without a wall thickness needs a closed profile (3+ points)".into());
                }
                Ok(())
            }
            Solid::Union(parts) | Solid::Difference(parts) => {
                if parts.is_empty() {
                    return Err("union / difference needs at least one solid".into());
                }
                parts.iter().try_for_each(Solid::validate)
            }
            Solid::Obj { scale, .. } => positive("obj scale", *scale),
        }
    }
}

/// A revolve profile resolved to (r, z, wall) points; `wall` is `None` for a
/// filled (closed polygon) profile.
pub(crate) struct Profile {
    pub points: Vec<[f64; 2]>,
    pub walls: Option<Vec<f64>>,
}

impl Profile {
    pub fn new(profile: &[Vec<f64>], thickness: Option<f64>) -> Self {
        let points = profile.iter().map(|p| [p[0], p[1]]).collect();
        let given: Vec<Option<f64>> = profile.iter().map(|p| p.get(2).copied().or(thickness)).collect();
        let walls = given.iter().any(Option::is_some).then(|| {
            // A point without a wall of its own takes the nearest given one.
            (0..given.len())
                .map(|i| {
                    given[i].unwrap_or_else(|| {
                        (0..given.len())
                            .filter_map(|g| given[g].map(|w| (g.abs_diff(i), w)))
                            .min_by(|a, b| a.0.cmp(&b.0))
                            .map(|(_, w)| w)
                            .unwrap_or(0.0)
                    })
                })
                .collect()
        });
        Profile { points, walls }
    }

    /// Profile of a tube's wall: a vertical mid-line.
    pub fn tube(outer: f64, inner: f64, height: f64) -> Self {
        let mid = 0.5 * (outer + inner);
        Profile {
            points: vec![[mid, -0.5 * height], [mid, 0.5 * height]],
            walls: Some(vec![outer - inner; 2]),
        }
    }

    /// Profile of a spherical shell: the mid-surface meridian, pole to pole.
    pub fn shell(outer: f64, inner: f64) -> Self {
        let mid = 0.5 * (outer + inner);
        let n = 96;
        let points = (0..=n)
            .map(|i| {
                let phi = std::f64::consts::PI * i as f64 / n as f64;
                [mid * phi.sin(), -mid * phi.cos()]
            })
            .collect();
        Profile { points, walls: Some(vec![outer - inner; n + 1]) }
    }

    /// A solid cylinder as a thick disc: the radius line, wall = height.
    pub fn cylinder(radius: f64, height: f64) -> Self {
        Profile { points: vec![[0.0, 0.0], [radius, 0.0]], walls: Some(vec![height; 2]) }
    }

    /// (r min, r max, z min, z max) including the wall.
    pub fn extent(&self) -> [f64; 4] {
        let mut e = [f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY];
        for (i, p) in self.points.iter().enumerate() {
            let half = self.walls.as_ref().map(|w| 0.5 * w[i]).unwrap_or(0.0);
            e[0] = e[0].min(p[0] - half);
            e[1] = e[1].max(p[0] + half);
            e[2] = e[2].min(p[1] - half);
            e[3] = e[3].max(p[1] + half);
        }
        e[0] = e[0].max(0.0);
        e
    }

    pub fn contains(&self, r: f64, z: f64) -> bool {
        match &self.walls {
            Some(walls) => self.points.windows(2).zip(walls.windows(2)).any(|(seg, wall)| {
                let (a, b) = (seg[0], seg[1]);
                let d = [b[0] - a[0], b[1] - a[1]];
                let len2 = d[0] * d[0] + d[1] * d[1];
                let t = if len2 > 0.0 {
                    (((r - a[0]) * d[0] + (z - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = [a[0] + t * d[0] - r, a[1] + t * d[1] - z];
                let half = 0.5 * (wall[0] + t * (wall[1] - wall[0]));
                q[0] * q[0] + q[1] * q[1] <= half * half
            }),
            None => {
                // Even-odd point in polygon, implicitly closed.
                let n = self.points.len();
                let mut inside = false;
                for i in 0..n {
                    let a = self.points[i];
                    let b = self.points[(i + 1) % n];
                    if (a[1] > z) != (b[1] > z) {
                        let x = a[0] + (z - a[1]) * (b[0] - a[0]) / (b[1] - a[1]);
                        if r < x {
                            inside = !inside;
                        }
                    }
                }
                inside
            }
        }
    }
}

/// (first radial, second radial, axial) coordinate indices for `axis`:
/// axis z is (x, y, z) = (r cos t, r sin t, z), axes x and y cycle them.
pub(crate) fn axis_frame(axis: Axis) -> [usize; 3] {
    match axis {
        Axis::X => [1, 2, 0],
        Axis::Y => [2, 0, 1],
        Axis::Z => [0, 1, 2],
    }
}

/// Coordinates (r, z) of `p` about `axis` through `center`.
pub(crate) fn to_rz(p: [f64; 3], center: [f64; 3], axis: Axis) -> (f64, f64) {
    let [a, b, c] = axis_frame(axis);
    let (u, v, w) = (p[a] - center[a], p[b] - center[b], p[c] - center[c]);
    ((u * u + v * v).sqrt(), w)
}

pub(crate) fn from_polar(r: f64, theta: f64, z: f64, center: [f64; 3], axis: Axis) -> [f64; 3] {
    let [a, b, c] = axis_frame(axis);
    let mut p = [0.0; 3];
    p[a] = center[a] + r * theta.cos();
    p[b] = center[b] + r * theta.sin();
    p[c] = center[c] + z;
    p
}

pub(crate) fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = super::mesh::sub(a, b);
    super::mesh::dot(d, d).sqrt()
}
