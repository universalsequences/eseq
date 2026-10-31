# Modal patches

Build Modalys-style physical-model instruments from objects and connections,
written as eseqlisp data with the `eseq.modal` module
(`content/ui/modal.lisp`) and compiled to an ordinary DGenLisp instrument.

```sh
eseq generate tools/modal/examples/cello.lisp [DIR]
```

This writes `dsp.lisp`, `ui.lisp` and `instrument.json` into DIR, by default
the user instrument folder under the patch's `:name`
(`.local/instruments/modal-cello` in a dev checkout). Check the result with
`eseq instrument check DIR`.

A patch is one kind of eseq.gen program, so C-c C-c in its buffer plays it on
a draft track, as any program does. The param, note-following, modulation
and panel machinery the generated code uses lives in `eseq.gen`,
`eseq.gen.modulate` and `eseq.gen.panel`, shared with other generators (see
tools/gen/README.md).

## A patch script

A script imports the module and ends with the patch:

```lisp
(import eseq.modal :as m)
(import eseq.modal.materials :as mat)

(m/patch :name "my-guitar"
  :objects     (list (m/finger 'fin :depth 0.1)
                     (m/string 'str :made-of mat/steel :length 0.65 :modes 60)
                     (m/plate 'body :made-of mat/spruce))
  :connections (list (m/pluck 'pick 'fin 'str 0.8)
                     (m/spring 'bridge 'str 0.96 'body (list 0.55 0.4)))
  :listens     (list (m/listen 'str 0.2 :gain 0.6)
                     (m/listen 'body (list 0.3 0.65) :gain 0.7)))
```

It is ordinary Lisp, so a patch can be computed: `examples/sympathetic.lisp`
bridges any number of tuned strings onto one body.

| Objects | Access point |
|---|---|
| `m/string` (stiff string, follows the note, `:tune` ratio) | `x` in 0..1 |
| `m/plate` (simply supported, `:grid` x `:grid` modes, default 8) | `(x y)` |
| `m/membrane` (circular, up to 120 Bessel modes, `:modes` how many, `:tune`, `:bend`) | `(r theta)`, knobs `x`/`y` across the face |
| `m/rect-membrane` (fixed edges, `:aspect`, follows the note) | `(x y)` |
| `m/circ-plate` (`:edge :free` or `:clamped`; cymbals, gongs; modes solved from the Bessel equations, pitch from material and size) | `(r theta)` |
| `m/bar` (free bar, follows the note; `:undercut` 1 tunes modes 2 and 3 to 4x and 10x, marimba-style) | `x` in 0..1 |
| `m/oscillator` (one mode at the note: a reed, a lip, a resonance) | `0` |
| `m/finger` (Modalys' two-mass: hand on a stroke + soft tip) | lumped |
| `m/mallet` (thrown mass) | lumped |
| `m/modal-data` (modes from data: frequencies, losses or bandwidths, shapes over points; `:root` makes it follow the note) | point index, or `(x y z)` for the nearest point |
| `m/net` (a tensioned network: nodes, masses, strands as `(i j tension/length)`, anchors; modes solved when generated) | `(x y z)`, the nearest node |
| `m/web` (a spider web built as a net: spokes, rings, an anchored frame, irregular, optionally a drum skin in its hub) | `(x y 0)`, the nearest node |

`(m/solid 'bell :shape (dict :revolve ...) :made-of mat/bronze)` is a
finite-element solid: any 3-D shape (the spec's "solid", below), solved for
its modes by `(modal-solve spec)` the first time it is evaluated (a few
seconds), then read from a cache keyed by the spec. Its shapes are scaled to
the same unit modal mass as every other object. `examples/fe-bell.lisp`
strikes a solved bronze bell; `:root` makes it follow the note.

**Named points.** A solid's `:points` names spots on it, in metres:
`:points (dict :bridge (list -0.06 0 0.0225) :lip (list 0 0.1 0.24))`. Any
connection or listen on that solid may then give a name for its position,
`(m/spring 'bridge 'str 0.985 'body :bridge)` or `(m/listen 'bell :lip)`,
and each snaps to the nearest surface node. Every named point is compiled
into the instrument, so a connection's or mic's "point" knob steps through
all of them (in name order), not only the ones the script uses. An unknown
name is reported by check with the names the solid has. Solids are composed
like any other objects: `examples/fe-bottle-guitar.lisp` glues a string to
fe-bottle's drilled bottle, and `examples/fe-hollow-body.lisp` builds a
semi-hollow electric guitar from a body and a neck solid joined at named
points, with pickups and a clean amp. `examples/fe-p-bass.lisp` is a
Fender Precision Bass the same way (an alder solid body with its routes, a
bolted one-piece maple neck), its four strings chosen by note, with the
fretted length, the core's stiffness and the split coil's position each a
`m/by-note` curve computed by the script. `examples/fe-tissue-box-guitar.lisp`
is the small version of that build (a cardboard box, a paper-roll neck and a
rubber band), coarse enough to solve in seconds: the one to learn the Shape
Lab on.

**Wood.** A solid made of a wood (`mat/maple`, `mat/mahogany`, `mat/spruce`,
`mat/rosewood`: materials with `:wood` constants) and given `:grain` is
orthotropic, as wood is: stiff along the grain and 10 to 20 times softer
across it. `:grain :x` runs the grain along x; `(list :x :z)` also puts
the growth rings' radial direction along z (quartersawn, as a guitar top
is). Without `:grain` a wood is isotropic at its along-the-grain modulus,
which makes a wooden plate ring several times too high. A bar solved both
ways comes out sqrt(E_L/E_T) = 3.9 times lower with the grain across it.
Woods also carry a loss factor (`:damping`, eta: maple 0.012, spruce 0.008):
a solid made of one loses pi·eta·f per second in each mode on top of `:loss`
and `:hf-loss`, so its high modes die ten times sooner than modes ten times
lower, as wood's do; metals have none and ring on `:loss`/`:hf-loss`. A
built instrument loses more than its wood (glue joints, radiation from the
top): give the solid its own `:damping` (the hollow body uses 0.03). Wood
damped like metal sounds like a bell. The "Damping x" knob multiplies all of
a solid's losses: higher dies sooner.

**Thin parts.** `:resolution (dict :cell-size (list x y z))` gives the voxel
cell's size along each axis, so a 3 mm top can be two cells thick at 6 mm
across: the element bends correctly in flat cells, and a bar meshed that
way matches cubes a quarter the size to 2%, with 1/16 the elements. A wall
thinner than one cell across (a 2 mm rim at 6 mm) still falls through.

**Where a solid sits.** `:place (dict :at (list x y z) :turn (list rx ry
rz))` puts a solid in the instrument: turned rx, ry then rz degrees about
x, y and z, then moved by `:at` metres. A solid without one is seated by
its glue: a spring between one of its named points and a placed solid's
puts the two points together (the hollow body's neck sits on the body this
way). Placement is geometry only for now: it draws the instrument and does
not change the sound.

**The shape lab.** M-x Shape Lab in a script with solids opens `*shape-lab*`
beside it (content/ui/modal/shape-lab.lisp), on the script's first solid
(tabs pick another):

- **Layers** (left): the shape as a tree, like Figma's: the body's parts
  (bronze), the cuts (red: holes, slots, arches), and what holds it (a
  clamp, or nothing). Selecting a layer selects it on the canvas; under
  the tree are its actions (**Duplicate**, **Repeat** into a row of N,
  **Delete**), its **Effect** (adds to the body or cuts into it: a union
  or a difference in the script) and its size and position. A revolve
  layer has **Edit profile**, which turns the canvas into its
  cross-section: drag its points, or set a point's radius, height and
  wall.
- **Canvas** (middle): the parts seen from the **Top**, **Front** or
  **Side**. A thin body is stretched to fill it (the toolbar says how
  much); a revolve seen side on shows its cross-section. Under it,
  **Move** drags parts and the clamp; **Hole**, **Slot**, **Block** and
  **Arch** (a marimba bar's undercut) each place one part where you click
  (straight through along your view, sized to the body; a new hole takes
  the last hole's radius), then hand back to Move. **Point** drops a named
  point on the surface under the click, on the side the view looks from;
  points are listed under **Points** in the layers, where you rename (Enter),
  snap, move or delete them, and Move drags one across the view and drops
  it back onto the surface (no re-solve: a point is not part of the shape).
  The armed tool's button is filled with its colour.
- **3-D view** (right): the shape as meshed, then, once solved, swinging
  through its lowest modes. Under it the mass, the lowest mode, and
  **Mesh**, the element size: voxel cells for most shapes, but a revolve
  with cuts keeps its fitted shell mesh (voxels cannot follow a thin wall)
  with the elements inside each cut removed, and there Mesh scales its
  `:segments`. A cut under two cells wide may vanish, and the lab says so.
- **Clamp:** a solid's `:block`, the region the solver holds still (where
  a cymbal sits on its stand), drawn green and dragged like a part. If an
  edit leaves it in empty space, its layer says "not touching" at once
  (the preview counts what it holds) instead of a solve failing, and
  offers **Move onto shape** or **Remove**; a free solid offers **Add a
  clamp**. Templates start free.
- **Instrument** (above the 3-D view; **This solid** goes back): every
  solid where it sits, tinted by material (the one being edited lighter),
  each string drawn between its two attachments and lifted off the
  surface, with pickups as bars under it, the pluck over it, mics and
  named points as markers. Drag to orbit; **Zoom** −/+. Under it, **Place**
  sets where the solid being edited sits (X Y Z, Turn X Y Z) and says
  whether it is placed, seated by its glue or at the origin; **Seat by
  glue** drops its position again.
- **Header:** the solid's tabs, **New shape…** (Bell, Bowl, Gong, Chime,
  Bar, Shell, Drilled, Marimba, Fork, Plate), the material and the mode
  count.

The geometry is content/ui/modal/shape-parts.lisp. Each edit is meshed at
once (`(modal-mesh spec)`, milliseconds) and solved in the background into
the cache `modal-solve` reads.

**Update draft** is one click: it writes `:shape`, `:made-of`, `:modes`,
`:resolution`, `:block` (removed when freed), `:points` and `:place` into the script's
`(m/solid 'id …)` call as one edit (one undo takes it back), evaluates the
script as C-c C-c does, and recompiles its draft; the button says
"Compiling draft…" until the draft plays the new shape, and an error shows
under the header. It waits for the solve, so the evaluation reads it from
the cache: a debug build takes about 20 s on the bell (40 modes); a drilled
bar 0.3 s at 32 cells, 2 s at 64, 9 s at 96 and 30 s at 128 (24 modes).
Lab coordinates snap to 0.1 mm, so the script reads back exactly what was
solved. A solid whose spec is a JSON file is shown but not written back.

`(modal-read-data "file")` reads data for `m/modal-data :data`, relative to
the script: an IRCAM resonance model (text: index, Hz, dB, bandwidth Hz per
line) or an `eseq-modes` `.json` from `eseq modal solve`.
`examples/bell-data.lisp` strikes a resonance-model bell. A data object with
more than 64 points keeps only the points its connections and listens use
(and its named points).

**Identified models.** A model fitted from a recording plays its measured
strokes instead of being struck. `tools/pm-ride/export_modes.py` writes the
ride behind the factory PM Ride Kit as `examples/data/ride.modes.json`: 688
modes with measured losses (`"measured": true`, so no air loss is added),
twelve strokes, each a complex residue per mode, and a wash of the
~1500 weaker modes as band noise. `examples/ride-kit.lisp` rebuilds the
factory instrument from it:

```lisp
(def ride (modal-read-data "data/ride.modes.json"))
(m/patch :name "modal-ride-kit" :voicing :one :level 2 :tone 20000
  :objects (list (m/modal-data 'ride :data ride :follow :octave))
  :connections (list (m/strokes 'stick 'ride))
  :listens (list (m/listen 'ride 0 :hear :position)))
```

- `(m/strokes id obj)`: the key picks a stroke (C, C#, ... in every octave)
  and the hit injects its residue into every mode on top of the ring. Its
  params: Hardness (a shorter or longer stick contact; a fixed energy
  budget), Vel > timbre, Click and Click tone (the stick's noise), Wash.
- `m/modal-data :follow :octave` transposes the object by the key's octave;
  `:muffle` (tape: loss rising with sqrt f) and `:mute` (a choke on release)
  act on the modes and the wash alike.
- `(m/listen obj at :hear :position)` hears displacement as it is (what the
  residues are), not the normalised velocity.
- `(m/listen obj at :radiate hz)` radiates less and less below hz (a
  two-pole high-pass, knob `mic<k>_radiate`): a board or body is a poor
  loudspeaker for wavelengths longer than itself, so a piano's lowest
  fundamentals are far weaker in the room than on the board.
- `(m/patch ... :voicing :one)` declares the host's voice mode (instrument.json
  `voice_controls`): every hit lands on one ringing plate. `:tone hz` adds an
  output low-pass. `:modulate :all` lets the host modulate every knob, at a
  cost (see Modulation).
- `(m/stroke-point data "Ride")` is the point where that stroke struck (the
  export infers each spot's mode shape from its residue), for physical
  connections there: `examples/ride-mallet.lisp` plays the measured plate with
  a felt mallet; `examples/ride-warp.lisp` warps its partials with
  `m/spectral`.

Rendered against the factory PM Ride Kit, the twelve strokes match in level
and in every band to 0.2 dB, and sample for sample to 0.3-1.6% RMS at the
recorded octave, retriggers included, at 8.6% of one core against the
factory's 9.8%. `examples/fe-ride.lisp` is the other road to a ride: its geometry,
solved by finite elements and shown in 3-D (240 modes, up to ~4 kHz).

A data object following the note (`:root`, `:follow`) computes its
coefficients when a note starts and when a param they depend on changes
(`coef_tick`), on DGenLisp's event clock, and latches them for the
per-sample code.

| Connections | Between |
|---|---|
| `(m/pluck id finger obj at)` | catch, hold, release |
| `(m/felt id mallet obj at)` | one-sided, hardening felt |
| `(m/spring id a a-at b b-at)` | bridge / adhere |
| `(m/bow id obj at)` | stick-slip friction, sustains under the gate |
| `(m/force id obj at :signal s)` | pushes the point with gain x signal |
| `(m/position id obj at :signal s)` | holds the point at the signal (a constant 0 clamps it) |
| `(m/speed id obj at :signal s)` | moves the point at gain x signal |

| `(m/strike id a a-at b b-at)` | stiff one-sided contact between two points (`:offset`) |
| `(m/stop id obj at :height h)` | a rigid floor under a point: sitar jawari, buzz |
| `(m/fingerboard id string at :press s)` | clamps the string while pressed, plus a board to buzz on |
| `(m/strokes id obj)` | plays a data object's measured strokes (see Identified models) |
| `(m/mount id a a-at string end)` | a string's end carried by another object, one way (see Electric) |
| `(m/snare-bed id head wires)` | snare wires (`m/snare-wires`) lying under a membrane, rattling against it (see Drums) |

`m/felt` takes `:exponent :hysteresis :tau-ms` for Stulov's hysteretic
felt (`examples/piano-hammer.lisp`).

**Drums.** `examples/snare.lisp` is a snare built from parts: a stick on
the batter head, the air inside the shell coupling it to the snare-side
head, and snare wires under that head. `tools/modal/bench/snare_bench.py`
scores it against the factory Membrane Snare, and `snare_fit.py` fits its
knobs to that.

- `(m/membrane 'batter :modes 32 :bend 1)` is a head. `:modes` is how many
  of its lowest modes it has (the cost is in proportion; 120 is the
  default, 24-32 is plenty for a drum, whose upper modes are too dense to
  hear one by one). Its coefficients are worked out when the note or a
  knob changes, not every sample. `Bend` is tension modulation: the head's
  own swing tightens it, so a hard hit starts sharp and glides down as it
  rings out (at Bend 1 a full-velocity hit starts about 12% sharp and
  settles in about 0.1 s; a soft hit hardly bends). Points on a head are
  written `(r theta)`; their knobs are `x`/`y` across its face, -1..1 from
  the centre.
- `(m/cavity 'air 'top 'bottom :radius :depth :modes)` is the shell's air
  between two `m/membrane`s, as the cylinder's acoustic modes (rigid walls;
  the overlaps of each with the head's modes are tabulated by
  `tools/modal/cavity_table.py` into `content/ui/modal/drum-tables.lisp`).
  The uniform mode is the air spring: pressing one head in pushes the other
  out, so the heads' (0,1) modes split into a pair, the heads moving
  together and, much higher, against each other. The others are masses of
  air on springs coupling the heads' (m,n) modes with the same m. Each is a
  spring between the volume the heads push into its shape and its air mass,
  solved centred in time, so the air conserves energy; `:top-density` and
  `:bottom-density` (kg/m^2: a 10 mil batter is 0.35, a 3 mil snare side
  0.1) set how hard it pushes each head, `:loss` its own damping, `Air` 0..2
  scales it. Both heads read positive upward. Verified: with every loss at
  0 a hit never gains energy, and the coupled peaks match an independent
  eigen-solve of the same heads and air.
- `(m/snare-wires 'wires :count :modes :pitch :spread)` is a bank of wires,
  each a string with its own few modes at a pitch of its own, spread a
  little so they do not lock.
- `(m/snare-bed 'snares 'head 'wires :points :span :width :gap :iterations)`
  lays them across the head's middle, each touching it at `:points` places:
  one-sided contacts solved each sample by `:iterations` passes of
  projected Jacobi over all the touches at once (a touch only ever pushes
  the head up and its wire down, each pass bounded so it never overshoots
  the force that closes the gaps, so a bed cannot add energy; fewer passes
  are a softer bed). `Gap` is the strainer, in the mics' units (a head
  mic's level for that swing: 0 lays the wires on the head); `Hard` softens
  the bed; `Drag` damps the head and wire against each other while they
  touch (the buzz's own loss, which ends it); `Snap` mixes in
  the contact force, high-passed (a mic beside the snares hears the slaps,
  not the wires' steady push), the buzz.

Both are solved in their own small spaces (air modes, touches) with the
coupling between them fixed when generating; the heads' shapes there leave
out the modes knob's mask. Cost goes with the touches times the head's
modes: `examples/snare.lisp` (8 wires x 2 touches, 32 + 24 head modes, 8
air modes) runs about 18x faster than real time per voice on an M-series
Mac, against 45x for the factory Membrane Snare and 1.4x for the same
drum with 120-mode heads, 32 touches and per-sample coefficients. DGen's
matrix-vector products cost about 0.3 us per thousand multiplies whatever
the formulation, so sparse couplings are gathered (`matvec-form`) and
nothing touch x touch is formed per sample.

**Autechre-style: string snares, webs, the drum pad.**
`examples/ae-snare.lisp` is built the way Autechre's Modalys snare patch
appears in a screen recording of it: a head, a stick, two mics, and two
"snares" that are heavy strings each touching the head at one point (a
`m/strike` contact), plus a bed of wires on the head for the buzz.
`examples/ae-snare-air.lisp` puts the same top on a real two-headed snare:
the shell's air (`m/cavity`) carries the hit to a snare-side head with the
wires under it (with Air at 0 the wires never rattle). `examples/web-drum.lisp`
hangs a drum skin in the hub of a spider web. The pieces:

- Modalys' pitch controls on `m/membrane` and `m/string`: given `:note`
  (a MIDI note), Note in (1 follows the played note, shifted by Shift
  semitones; 0 sits at Note), Shift and Note knobs. `:shift 12` puts a head
  an octave over the note.
- `m/membrane :made-of mat/water :thick-mm 3`: a skin with thickness has
  bending stiffness on top of its tension, stretching its upper modes
  sharp in proportion to E h^2 / rho (with the tension set so the
  fundamental stays on its pitch); a stiff or thick head clanks. `:radius`
  sets the head's size. New materials: `mat/mylar`, `mat/water` (its bulk
  modulus as Young's: a what-if, as Modalys allows), `mat/spider-silk`.
- `m/string ... :mass 100`: a heavy string, which a head slaps but hardly
  moves, so the head buzzes against it. A string given `:note` has its
  coefficients worked out when the note or a knob changes.
- `m/strike ... :mic-gap true`: the contact's Gap in the mics' units (as a
  snare bed's), so it reads against what is heard.
- `(m/listen obj at :left 1 :right 0)`: any listen with a left or right
  level makes the instrument stereo (Modalys' L/R matrix: listen to the
  snares themselves as well as the head).
- `(m/patch ... :out-filter (dict :type :highpass :cutoff 110 :q 0
  :steep false))`: an output filter, low-, band- or high-pass, Q, and a
  24 dB/oct switch.
- `(m/patch ... :pad 'head)`: the panel gets a drum pad, the head as a disc
  with a numbered dot for every point on it: the stick (blue), contacts
  with other objects (grey), mics (green). Drag a dot to move that point
  (its x/y knobs). It is the `drum-pad` widget (`:count`, `:x1 :y1` ..
  `:x8 :y8` in -1..1 with y up, `:labels`, `:colors`, `:on-change (k x y)`).
- `m/net` and `m/web`: a tensioned network (point masses joined by strands
  under tension, moving across its plane) is solved once when the
  instrument is generated: K is the strands' graph Laplacian (tension over
  length per strand), M the node masses, and `modal-net-solve` (Rust,
  `crate::modal_fe::net`) eigen-solves it dense, so a few hundred nodes
  take milliseconds. Then it is a data object: it costs its mode count, not
  its node count (the web drum's 64 modes run about 22x faster than real
  time). `m/web` lays out a hub, `:spokes` out to an anchored frame and
  `:rings` of capture thread, of `:made-of` silk `:thread-um` thick under
  `:spoke-tension` and `:ring-tension` (N), `:irregular` (seeded jitter,
  as real webs are) and `:bead` mass per node; `:hub` (m) stretches a skin
  of `:hub-tension` (N/m) and `:hub-density` (kg/m^2) over the middle, so a
  drum skin hangs on the threads as springs. Its pitch follows the note by
  its lowest audible mode (a heavy hub also bounces whole on the threads,
  far lower). Reshaping a web means generating again; the drum pad does not
  place points on a net yet.

**Following the note.** Any knob argument can be a curve over the played
pitch instead of one number: `(m/by-note 27.5 2.0 262 0.66 4186 0.05)`
(pitch value pairs, interpolated over log pitch: geometrically when every
value is positive). The knob, `str_length_c4`, sets the value at C4 and moves
the whole curve with it. Real instruments are scaled this way: a piano's
strings shorten from 2 m to 5 cm and its hammers lighten up the keyboard; a
cello's wound strings keep one stiffness over its range. A string's pitch
comes from tension, so a fixed length and radius make its stiffness go as
1/f0^2: the bass of a one-size piano is eight times too inharmonic (clangy,
tinny) and a one-size cello cannot bow its low notes cleanly.

**Unison.** `(m/string 'str ... :unison 3 :detune 1.5)` is a piano's three
strings: copies sharing every param (one panel section, a Detune knob in
cents), each connection to `'str` made to every copy (the hammer strikes
them all, the bridge carries them all). Through a shared bridge they decay
in two stages, the in-step motion draining into the board and the
out-of-step motion ringing on (Weinreich's aftersound). A listen hears the
first copy.

**The piano** (`examples/piano.lisp`) is fitted to the 30 Salamander Grand
references PM Piano is calibrated from, by PM Piano's own metric
(`tools/pm-piano/compare.py`'s partial-band error) plus the envelope and the
spectral centroid: scale, gauge, hammer, felt, strike point, losses, detune
and the board's mass ratio are each an `m/by-note` curve through the fit.

**Directions.** `(m/string 'str :planes 2)` adds a second transverse
direction, `(m/plane 'str 1)`, with the same params (`split` detunes it
slightly). A bow with `:hair d` on such a string presses the second
direction down, and its friction pressure follows that contact's force
(`examples/bowed-bi-string.lisp`).

**Feedback taps.** Signals that read last sample: `(m/tap-position obj at)`,
`(m/tap-speed obj at)`, `(m/tap-force conn)`, `(m/tap-contact conn)`, and
`(m/speaker)` (an amp's sound; see Electric). A
force's `:saturate` bounds a self-driven loop (`examples/feedback-gong.lisp`).

**Hybrids.** `(m/melt 'h objA objB :morph x)` blends two objects' modal data
(equal mode counts; positions on it are `(list atA atB)`):
`examples/melt.lisp` morphs a string into a membrane. `(m/mix-hybrid 'h 'a 'b)`
shares connections and listens between two objects in the patch
(`examples/mix.lisp`).

**Air.** `(m/tube 'bore :ends :closed-open)` (also `:open-open`,
`:closed-closed`, `:cone`) is an air column on the note. Blown connections
take a `:breath` signal and drive the tube by the flow's change, the way an
air column's input impedance does:
- `(m/reed 'reed 'bore 0 :blow 0.7)`: the normalised clarinet reed (speaks
  above blow ~1/3; `examples/clarinet.lisp`)
- `(m/lips 'lips 'lip 'bore 0)`: a lip oscillator (`m/oscillator`) buzzing
  into the bore (`examples/brass.lisp`)
- `(m/jet 'jet 'bore 0.05)`: a flute jet, delayed and saturated
  (`examples/flute.lisp`; it favours overblown registers, eseq-k0m2.33)
- `(m/tone-hole 'h 'bore 0.66 :open 1)`: opening it shortens the column

**Electric.** A guitar's pickup, its amp, and the air between the amp and
the guitar, so the amp's sound can drive the strings (feedback):

```lisp
(m/patch :name "modal-eno-lead" :voicing :one
  :objects (list (m/finger 'pick :depth 0.06 :stroke-ms 6)
                 (m/string 'str :made-of mat/steel :radius-mm 0.33 :const-loss 0.1 :freq-loss 0.06 :mute 30)
                 (m/plate 'body :made-of mat/maple :thick-mm 10 :mass 30 :loss 40 :damp 3))
  :connections (list (m/pluck 'plk 'pick 'str 0.82)
                     (m/force 'air 'body (list 0.4 0.6) :gain 0.003
                              :signal (list (m/speaker :distance 2 :angle 60 :room 0.5)
                                            (m/envelope :attack-ms 5 :decay-ms 1 :sustain 1 :release-ms 60)))
                     (m/mount 'bridge 'body (list 0.5 0.45) 'str 1))
  :listens (list (m/pickup 'str 0.92 :height 3 :coil-hz 2600))
  :amp (m/amp :drive 30 :bias 0.25 :bloom 0.6 :presence 0.8))
```

- `(m/pickup obj at :height h :coil-hz f :coil-q q)`: a listen that hears the
  string as a magnetic pickup does: its speed times how fast the flux
  changes with its distance from the pole, which grows as the string comes
  closer (louder, asymmetric, more even harmonics; `height` is in units of
  a firm pluck's swing), through the coil's resonant low-pass.
- `(m/patch ... :amp (m/amp ...))`: the mix plays through an amp inside the
  instrument before the output. Fuzz (drive into tanh about a bias; the
  input's peaks charge its coupling capacitor, drifting the bias by a share
  of the swing, so the clipping stays lopsided however hard it is driven:
  Bloom; antiderivative-antialiased), a bass / mid / treble stack, a power
  stage whose drive sags with its load, then the speaker cone with its
  presence peak and the cabinet. `:low-cut`, `:cone-hz` and `:bass-hz` (fixed
  when generating; 80, 100 and 120 Hz) are where its input coupling, speaker
  cone and Bass band sit: a bass amp's are an octave lower
  (`examples/fe-p-bass.lisp`), or it cuts away a bass's 41 Hz low E.
- `(m/speaker :distance m :angle deg :room r)`: a signal for any driver. It
  is the speaker's sound at that distance (delayed by the air, falling as
  1 / distance), off-axis angle (a 12-inch cone beams, so off axis it loses
  its top), plus the room's reflections off the floor and two walls, each
  along its own, longer path.
- `(m/mount id a a-at string end)`: a string's end (0 or 1) mounted on a
  point of another object, one way, as a solid body carries its strings'
  bridge: a base excitation, each mode pushed by its share of the end's
  motion, 2 / (n pi), times the mount's acceleration. The string moves
  relative to the mount, as a pickup on the body hears it, and a solid body
  takes almost nothing back, so the string keeps its sustain. (A bridge
  spring that near the end pins the string, scattering energy into
  fast-decaying high partials; a force from the body's displacement leaks
  the body's own resonance to the pickup, which then howls on through a
  muted string.)
  `:back` (experimental) adds the string's end force on the body (its
  tension along its end's slope, m Σ s_k ω_k² q_k). It is not yet
  energy-conserving: with a hollow body's masses it pumps partials near
  the body's or neck's modes and runs away on high notes (eseq-lojj.22),
  so leave it off.

`examples/eno-lead.lisp` sustains held notes and then blooms into
feedback. Air > Distance picks the partial (fundamental, octave, twelfth,
double octave), Air > Gain is how much sound the guitar catches, Air > Room
how many spots feed back, and the amp's clipping holds the loop. Releasing
the key is the player's hand on the strings: the envelope on the air closes
the loop and the mute stops the string within ~0.1 s. Keep `:voicing :one`:
in poly mode every voice gets its own amp.

`(m/spectral obj ...)` adds Modalys' pseudo-physical controls to any object:
`bend` (cents) and `diffraction` (rho: 1 leaves the partials, below 1 pulls
them toward the lowest, above 1 spreads them) params, `:follow-note` (the
lowest mode on the played note, for plates), and per-mode lists
`:freq-scales`, `:loss-scales`, `:gains` (a mode equalizer on what listens
hear). `(m/listen obj at :modes (list 0 2))` hears only those modes.

Every connection has a `weight` param (0..1) that scales its force (a bow's
pressure), to fade it in or out or couple partly. A driver's signal is data:
`(m/envelope :attack-ms :decay-ms :sustain :release-ms)`, `(m/noise)`,
`(m/sine :ratio r)` (at r times the note), `(m/constant v)`, or any
DGenLisp form `(m/signal '(...))`; a list of signals is their product.
`examples/ebow.lisp` sustains a string with a sine force at the note.

Materials are data. `eseq.modal.materials` (`content/ui/modal/materials.lisp`)
holds a library (`mat/steel`, `mat/nylon`, `mat/spruce`, `mat/bronze`,
`mat/glass`, ...), each a dict of name, density, Young's modulus and
Poisson's ratio. A string or plate is `:made-of` one, or a list of them to
pick from on the panel (the first is the default):

```lisp
(m/plate 'body :made-of (list mat/spruce mat/maple mat/rosewood))
(m/string 'str :made-of (mat/material :name "gut" :density 1300 :young 5e9))
(m/string 'str :made-of (merge mat/steel :name "spring steel" :young 2.1e11))
```

(The argument is not called `:material`: every call quotes the value after
`:material`, which carries shader code for widgets.)

`(m/check patch)` lists what is wrong with a patch by name (`spring bridge:
no object bdy`); `eseq generate` prints the same list instead of writing
anything.

Every param is named after its object or connection (`str_length`,
`bridge_stiff`) and grouped by it (`@group str`), so the auto-generated panel
shows one section per object, and the generated `ui.lisp` (addressing params
as `str.str_length`) gives each a knob panel around a screen that draws the
patch and lists the selected section's params.

### Modulation

The host's four modulation sources can drive a patch's knobs (`@mod true`,
read as `(mod p)`). Which knobs depends on what they cost:

- By default (`:modulate :cheap`), every knob whose work runs anyway when
  it moves: a bow's pressure, a string's loss or length, a felt's
  hardness, gains, envelopes, the amp, and the by-note knobs and a data
  object's tune / decay, whose coefficients are worked out on a change tick
  (`bynote_tick`, `coef_tick`). This costs nothing until a knob is
  modulated (the examples are within a few percent of unmodulatable).
  While a source moves, those ticks recompute every 32 samples (0.7 ms).
- Not by default: knobs that only shape setup DGen works out once per block
  from knobs alone (a plate's size, thickness, mass and loss; a point on a
  plate, such as a mic's or a bridge's position), or that are mixed with a
  latched coefficient. `(m/patch ... :modulate :all)` makes them
  modulatable too: their setup reruns every 32 samples while a source
  moves, and its results are latched every sample whether modulated or
  not: the cello +20% (7.0% -> 8.4% of one core per voice), the piano
  +80%, the Ride Kit +110% of their CPU.

Modulating a knob by d sounds exactly like moving it by d (to the sample).

## What the generated DSP does

Each sample follows the protocol of the `content/defmacros/modal-*` macros:
predict every object's free motion; solve the exciters (pluck, felt) in turn;
solve springs and bows together by Gauss-Seidel sweeps; commit every object.
All forces are solved inside the sample from both ends' compliance, so any
stiffness is stable. A spring is centred in time (unless an end is a mallet or a mix)
(`spring-centred`): a force acting on the next sample's gap alone takes a
factor 1/sqrt(1 + kC) out of every mode at the point each sample, and kC
grows with pitch squared, so bridges drained treble strings in a fraction of
a second whatever their stiffness.

## Rules learned the hard way

- Library macros take and return state values; the patch owns its
  `make-history` / `read-history` / `write-history`. A macro in
  `content/defmacros` that reads or writes a history passed in by name
  compiles, but the patch editor rejects it and that breaks the whole patcher.
- Contacts must not create energy: a pluck engages its spring at the gap it
  caught at, and a bow reads its point's true previous position.
- Connections sharing an object are solved jointly; one sequential pass let
  stiff springs on a light body, and a bow next to a bridge, pump energy in.
- Bowing needs harmonic modes: keep the string's stiffness low at low notes
  (a radius following the note) and put bridges at the string's very end
  (0.999): a stiff point at 0.99 pins it a second time. The bow's slip below
  about 2 whistles on the 3rd or 4th harmonic, a woodwind's sound, instead
  of Helmholtz motion (the default is 4).
- A body's lowest mode belongs near its real one, well damped: a 0.75 m
  plate of 4 mm spruce rings at 60 Hz and wolfs a cello's whole low octave.
- Masses are relative to one modal mass; a 120-mode head's point is ~60x
  lighter, so useful mallets are 0.002..0.5.
- Modulation and DGen's scheduling (mod-pass in `content/ui/modal.lisp`):
  an event clock must not read `(mod p)` (each is gated on its routes being
  active, and a clock in a gated cone miscompiles what it clocks), so the
  ticks watch the knobs and the raw sources; work on a tick reads
  `(event-hold (mod p) tick)`, never a bare `(mod p)` (it would run every
  sample) nor another clock's latch (garbage between that clock's events: a
  data bell clicked on release); and pure math over a latch is scheduled on
  its trigger, so a modulated value mixed into it (a bell's G from its
  latched wref and its mass) silences the instrument.

## Finite-element solids (mesh → modes)

`eseq modal solve SPEC.json OUT.modes.json` meshes a 3-D elastic solid and
computes its lowest vibration modes (`crates/sequencer/src/modal_fe/`).
Samples: `fe/bar.json` (free aluminium bar, voxel mesh) and `fe/bell.json`
(bronze bell revolved from its wall profile, mapped mesh).

A spec names a `solid` (`box`, `cylinder`, `tube`, `sphere`, `shell`,
`revolve` from `[r, z]` / `[r, z, wall]` profile points, `union`,
`difference`, or an `obj` file), a `resolution` (`{"cells": N}` along the
largest extent, `{"element_size": metres}`, or `{"cell_size": [x, y, z]}`),
a `material` (`density`, `young`, `poisson`, and for wood a `grain`:
`along` and `across` axes with `young_r`, `young_t`, `shear_lr`,
`shear_lt`, `shear_rt`, `poisson_lr`, `poisson_lt`, `poisson_rt`), an optional `block` region (`box` min/max
or `sphere` center/radius) whose nodes are clamped, and the number of
`modes`. A lone revolve with a wall, tube, shell or cylinder gets a
body-fitted mesh (`"segments": {"along", "around", "through"}` override its
counts); anything else is voxelized.

The output is the "eseq-modes" v1 JSON document: `freqs` (Hz, ascending,
rigid-body modes excluded), the boundary surface as `points` / `normals`
(P×3) and `triangles` (T×3), `shapes` (P×N, point-major: mode k's
mass-normalized displacement along the normal at point p is
`shapes[p*N+k]`), `mass` (kg) and the canonical `spec`.
