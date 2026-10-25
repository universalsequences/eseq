# eseq.modal vs IRCAM Modalys — gap analysis

Status: rev 1, 2026-10-24. Branch `modalys-next`. Epic eseq-k0m2 (protocol
macros, `eseq.modal` generator, C-c C-c drafts) is the baseline.

Sources: the official Modalys manual, 4.0.0 b3 (Oct 2026) and 3.6.1 (2021),
read in full from the Wayback Machine because support.ircam.fr/docs/Modalys
was down for maintenance; the forum release notes (3.1–3.5 PDF and later
threads); and the IRCAM papers (Iovino, Dudas, Caussé et al. 1997/1998;
Bensoam 2003–2008 abstracts on HAL). Page-level citations and the extracted
text are in the two research reports this rev was written from; the
specifics below that the manual does not state are marked (inferred).

## 1. The one-paragraph answer

Modalys is a *construction kit*: about 25 object types, about 20 interaction
types, a controller algebra that drives every parameter, modal-domain
hybrids, imported or finite-element modal data, and a 3-D mode viewer. Most of
its depth is in **breadth of objects and interactions** (above all the air
family: tubes, reeds, lips, flutes) and in **data-defined objects** (measured
modes, FE meshes). eseq.modal has the same core idea (linear modal objects,
nonlinear connections, contact-microphone listening) with a small catalogue,
but it is ahead where Modalys is weak: it is polyphonic per voice, compiled to
native code, solves every connection implicitly inside the sample (energy-safe
at any stiffness), and is edited live as Lisp with a generated panel. The gap
is mostly *catalogue*, not *architecture*, which is the good news.

## 2. Where we already match or lead

| | eseq.modal | Modalys |
|---|---|---|
| Object model | per-mode two-pole resonators, unit modal mass, G = w² coupling | per-mode modal data, same idea |
| Loss | const + freq-proportional + fixed f² air loss (s⁻¹) | const + freq·(f/kHz)² (s⁻¹) |
| Solver | every connection implicit in-sample; Gauss-Seidel sweeps over springs/bows sharing objects | piecewise-linearised active-set solve, later Gauss-Seidel (Bensoam 2008); force applied next sample in the 1998 description |
| Moving accesses | access positions are params, so modulation moves them per sample | access location is a controller |
| Listening | velocity at points (contact mic), several mics mixed | point-output = access speed |
| Polyphony | per voice, host voice allocation, voices stop when silent | none: `poly~` with one engine per voice |
| Speed | DGenLisp compiled to native SIMD code | interpreted engine, `set-precision 'float` is the speed knob |
| Authoring | Lisp script → generated DSP + panel, C-c C-c drafts, hot recompile keeping knob values | Max patches, Lua, ModaLisp (offline) |
| Control | every param is a host param: knobs, p-locks, mod matrix, LFOs/envelopes | controllers (§3.4) |
| Bow friction | rosin-like μ(x) curve, implicit root with stick/slip hysteresis | 3-segment piecewise-linear curve, 4 non-physical coefficients |

## 3. Gaps

Depth key: **●** missing entirely, **◐** partial, **○** small difference.

### 3.1 Objects

| Modalys | Ours | Gap |
|---|---|---|
| mono-string | string (one transverse direction) | ○ |
| bi-string (2 independent transverse directions) | — | ● needed for a real bow (one direction carries the normal force) and fingerboard |
| harmonic-oscillator (1 mass + spring) | — | ● trivial; the usual reed/lip |
| mono-two-mass / bi-two-mass | finger (bi-two-mass-like, one direction), mallet | ◐ no two-direction variant; masses are not general drivable objects |
| rect-plate | plate: simply supported, fixed 8×8 = 64 modes | ◐ mode count fixed |
| clamped-circ-plate, free-circ-plate | — | ● cymbals, gongs, bells (approximate) |
| rect-free-bar (orthotropic wood, free edges) | — | ● marimba, xylophone, vibraphone, glockenspiel |
| rect-membrane | — | ● |
| circ-membrane | membrane: 120 Bessel modes, clamped | ○ |
| closed-closed / closed-open / open-open tube (cylindrical or conical) | — | ● the whole wind family depends on it |
| violin-bridge, cello-bridge (lumped bridges) | — | ● spring + plate stands in |
| jet (nonlinear waveguide) | — | ● flutes, recorders, organ flues |
| single-point (freqs / bandwidths / amps lists) | — | ● data-defined object |
| multiple-points (N modes × P points shape table) | — | ● the "core modal object" everything else reduces to |
| reson-model (IRCAM modres file) | — | ● measured resonances |
| read-from-file / save-object, clone | — | ● |
| finite-element (3-D mesh → modes) | — | ● see §4 |
| melt-hybrid, mix-hybrid, tri-hybrid | — | ● morph between objects |
| radiator (spherical-cap radiation impedance) | — | ● |
| hanging-chain, waveguide tubes (4.0) | — | ● minor |

### 3.2 Connections

| Modalys | Ours | Gap |
|---|---|---|
| force, speed, position (external drivers) | — (finger and mallet drive themselves) | ● nothing can be pushed by a signal or envelope; no clamping |
| adhere (velocity equality, works across physical domains) | spring at high stiffness (displacement-based) | ◐ no cross-domain (string → tube) coupling |
| spring (damped oscillator between points) | spring (stiffness only) | ○ no own frequency/loss |
| viscous-damping (4.0) | — | ● trivial |
| strike (rigid one-sided contact; unilateral against a ground) | — (felt is the only strike) | ● buzz, tampura/sitar jawari, snare wires |
| pluck (force-threshold release) | pluck | ○ |
| felt (Stulov: power law + hysteresis: thickness, F0, α, ε, τ) | felt: one-sided, stiffens with squash | ◐ no hysteresis or power law |
| bow (4 accesses: bow normal + tangent, string normal + tangent) | bow on one direction, pressure is a param | ◐ pressure cannot come from the physics |
| mono-/bi-fingerboard (finger clamps string on a virtual board) | — | ● fretting, slides, buzz on the board |
| reed (Backus flow, beating against a lay) | — | ● clarinet, sax |
| valve, leaky-valve, normalised-valve, normalised-leaky-valve | — | ● brass lips, voice-like |
| labium (with jet object) | — | ● flutes |
| hole (tone hole = tiny tube, adhered with a weight) | — | ● |
| induction (electromagnet, gap-dependent) | — | ● e-bow / magnetic resonator piano |
| weight on every connection (fade or partially enable) | — | ● cheap and useful: one param per connection |

### 3.3 Accesses and output

| Modalys | Ours | Gap |
|---|---|---|
| directions per object (trans0/trans1/long/normal/...) | one direction per object | ● comes with bi-string / tubes |
| `'decoded` weight vectors: area accesses and outputs | points only | ◐ |
| compiled accesses (precomputed points) | phi computed per sample from the param | ○ (ours is dynamic; theirs is cheaper) |
| point-output mode selector (listen to chosen modes) | — | ● cheap |
| output any controller (position, force as audio) | velocity only | ◐ |
| radiator impedance | — | ● |

### 3.4 Control

| Modalys | Ours | Gap |
|---|---|---|
| every physical parameter is a controller | every param is a host param (mod, p-lock) | ○ |
| per-mode frequency / loss / amplitude vectors ("mode equalizer") | — | ● |
| cents pitchbend, spectral diffraction (cheap spectral warp without recomputing) | — | ● our objects already recompute freqs per sample, so these are formula changes |
| set-pitch: tune by solving for one physical parameter | strings and membranes follow the note; plates never do | ◐ |
| feedback taps: access-position / -speed / -force, connection-state → drive another connection | — | ● sympathetic loops, self-oscillation, nonlinear feedback |
| audio-in signal controller | — | ● depends on instrument audio-in |
| controller algebra (envelope, sine, phasor, noise families, delay, filters, arithmetic, dimension routing, Lua) | host modulation + DGenLisp itself | ○ different design; DGenLisp covers it |
| MIDI-file and sound-file controllers | sequencer | ○ |

### 3.5 Data and materials

| Modalys | Ours | Gap |
|---|---|---|
| material tables: ~60 metals, minerals, fibres and others; ~30 woods with three grain-direction Young's moduli; air-density table | materials are data (`eseq.modal.materials`): 19 isotropic materials, user materials and variants, per-object menus | ○ more entries; grain directions wait for an orthotropic object (the free bar) |
| measured modal data import (modres text, saved objects) | — | ● |
| FE mesh tooling, .obj import, multi-domain materials | — | ● §4 |

### 3.6 Tooling

| Modalys | Ours | Gap |
|---|---|---|
| Medit 3-D viewer: mesh, blocks, animated mode shapes, node picking | patch drawing on the generated panel | ● §4.3 |
| CLI with float regression compare | `eseq modal emit`, `eseq instrument check`, audition | ○ |
| bundled instruments (snare, bass drum, cymbal, hi-hat, eolian harp, marimba, vocalys) | 6 examples (cello, guitar, drum, banjo, sympathetic, dulcimer) | ◐ |

## 4. The 3-D / finite-element track

### 4.1 What Modalys does

- Meshes are built procedurally (points extruded to lines, quads, hexahedra;
  translate / rotate / scale / reflect; restrict to sub-meshes) or imported
  (INRIA `.mesh`, `.obj` since 3.6). Elements: hexahedra (Lisp); beams and
  quad shells with thickness in the 4.0 Lua API.
- A *block* sub-mesh is clamped (the boundary condition); without one the
  object drifts.
- `compute-modes` runs a sparse eigensolve (Eigen + Spectra), asynchronously
  in 4.0. Materials can be multi-domain and anisotropic.
  `get_pitched_finite_element_object` scales the mesh until a chosen mode
  hits a target note.
- Accesses are mesh nodes (normal, or tangent toward a second node, or the
  nearest node to x, y, z). Every connection except `hole` works on FE
  objects.
- Medit animates any mode shape and picks nodes to place accesses.

### 4.2 The key observation

A finite-element object, once its modes are computed, *is* a
`multiple-points` object: N mode frequencies, N losses, and an N × P table of
mode shapes over the mesh nodes. So the runtime half of FE is the same as the
data-defined object in §3.1, and nearly all the FE work is offline:

1. **Data-defined modal object** (runtime): `(m/modal-data 'bell :file
   "bell.modes")` carrying freqs, losses and a shape table; access =
   node index (or nearest node to a point). Also serves measured data
   (modres import) and the analytic objects we lack.
2. **Mesh → modes** (offline, CLI and app worker): build or import a mesh,
   pick a block, assemble stiffness/mass, sparse eigensolve, write the
   `.modes` file. Rust has the pieces (sparse matrices + a Lanczos
   eigensolver); this runs once per object, not per voice.
3. **Mesh view** (UI): §4.3.

Open question for step 1: how a DGenLisp instrument reads a shape table from
its folder (an asset tensor), and how large it can be: an N × P table is
read only at the access nodes, so the DSP needs just the rows of the nodes
in use if access nodes are fixed at generation (Modalys' compiled
accesses), or the whole table if they move.

### 4.3 Mesh view: a vertex stage for `defwidget`

`defwidget` today is a full-quad fragment shader; a mesh is vertex-centric.
Proposal: give `defwidget` an optional vertex stage written in the same Lisp
shader DSL.

```lisp
(defwidget mode-mesh
  :geometry :mesh            ; vertices + indices, uploaded once, keyed
  :state (amps :array)       ; this frame's mode amplitudes (eseq-nu3h)
  :vertex
  (let ((u (dot amps (mode-row vertex-id)))      ; displacement = Σ a_k φ_k
        (p (+ position (* normal u scale))))
    (out :position (orbit-project p yaw pitch) :varying (vec2 u (dot normal light))))
  :shader
  (mix :widget-mesh-base :widget-mesh-hot (abs (varying 0))))
```

- The DSL codegen gains a vertex entry point: typed attributes in
  (position, normal, vertex index), clip position and a few varyings out;
  uniforms and helpers are shared with the fragment stage.
- Geometry widgets render into their own offscreen texture with a depth
  buffer, composited by the existing image pipeline, so the 2-D renderer's
  CPU-positioned vertices stay as they are everywhere else.
- eseq-nu3h (array states) becomes the way per-frame data reaches the vertex
  stage: mode amplitudes tapped from a running voice (the OTT meter pattern:
  node state → widget).
- Strings, plates and membranes are generated geometry (line, grid, disc),
  so one widget shows every object; the same feature gives 3-D scopes and
  waveform terrains.

To decide: triangles first, lines next; camera as uniforms vs a built-in
orbit; geometry size limits; which voice the amplitudes come from.

## 5. Proposed order

Each tier builds on the previous; within a tier items are independent.

**Tier 1: cheap, high value, no new solver work** (done 2026-10-25: data
objects and modal-read-data; bar, rect-membrane, circ-plate, oscillator,
plate grid; force / position / speed drivers with signals and a weight on
every connection; m/spectral and listen :modes; see tools/modal/README.md)

1. Data-defined objects (`single-point`, `multiple-points`, modres import):
   the runtime half of FE and of measured instruments.
2. More analytic objects: free bar (marimba family), clamped / free circular
   plates, rectangular membrane, harmonic oscillator; a mode count param on
   the plate.
3. Driver connections (force / position / speed) and a weight param on every
   connection.
4. Spectral controls: per-mode frequency / loss / amplitude, cents
   pitchbend, spectral diffraction; set-pitch for plates and bars.
5. ~~Material table~~ done as materials-as-data (eseq-k0m2.13); grain
   directions come with the free bar.
6. Point-output mode selection.

**Tier 2: new physics on the existing solver**

7. Directions: bi-string and two-direction masses; then a bow driven by a
   bow object's normal force, and the fingerboard.
8. Strike (rigid one-sided, unilateral against a ground) and Stulov felt.
9. Feedback taps: access position / speed / force and connection state as
   sources for driver connections (sympathetic loops, self-oscillation).
10. Hybrids: melt (interpolate modal data) and mix (split energy).

**Tier 3: the air family**

11. Tubes (cylindrical and conical, three end conditions), adhere across
    domains, holes.
12. Reed, then valve (lips) and its leaky / normalised variants.
13. Jet + labium (flutes).

**Tier 4: 3-D**

14. Mesh → modes offline pipeline (§4.2 step 2) on top of tier-1 item 1.
15. `defwidget` vertex stage and the mode-mesh view (§4.3), after eseq-nu3h.

Each item also wants a bundled example instrument in `tools/modal/examples/`.
