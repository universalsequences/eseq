#!/usr/bin/env python3
"""Write small "eseq-modes" v1 fixture files without the finite-element solver.

The format (what the solver writes, and mode-mesh-view / (mesh-load) read):
  {"format": "eseq-modes", "version": 1, "freqs": [N], "points": [P*3 metres],
   "normals": [P*3], "triangles": [T*3 indices], "shapes": [P*N point-major:
   shapes[p*N+k] = mode k's displacement along the normal at point p],
   "mass": kg, "spec": {...}}

  plate  a simply supported plate on a grid, analytic sin(m pi u) sin(n pi v)
  bell   a bell-like shell of revolution (a real 3-D mesh with varying
         normals), with made-up cos(m theta) shapes that grow toward the rim

Usage: scripts/gen_modes_fixture.py plate|bell OUT.json
"""

import json
import math
import sys


def plate(nx=16, ny=12, lx=0.6, ly=0.4, count=6):
    modes = sorted(((m, n) for m in range(1, 6) for n in range(1, 6)),
                   key=lambda mn: (mn[0] / lx) ** 2 + (mn[1] / ly) ** 2)[:count]
    f0 = (modes[0][0] / lx) ** 2 + (modes[0][1] / ly) ** 2
    points, shapes = [], []
    for j in range(ny + 1):
        for i in range(nx + 1):
            u, v = i / nx, j / ny
            points += [lx * (u - 0.5), ly * (v - 0.5), 0.0]
            shapes += [math.sin(m * math.pi * u) * math.sin(n * math.pi * v) for m, n in modes]
    triangles = []
    for j in range(ny):
        for i in range(nx):
            a = j * (nx + 1) + i
            triangles += [a, a + 1, a + nx + 2, a, a + nx + 2, a + nx + 1]
    freqs = [200.0 * math.sqrt(((m / lx) ** 2 + (n / ly) ** 2) / f0) for m, n in modes]
    normals = [0.0, 0.0, 1.0] * (len(points) // 3)
    density, thickness = 7850.0, 0.002
    spec = {"object": "plate", "lx": lx, "ly": ly, "thickness": thickness,
            "boundary": "simply-supported", "modes": [list(mn) for mn in modes]}
    return freqs, points, normals, triangles, shapes, density * lx * ly * thickness, spec


def bell(rings=18, segments=40):
    # Profile from the crown (s=0) to the mouth (s=1): radius and height, metres.
    def profile(s):
        return 0.05 + 0.07 * s + 0.06 * s ** 3, 0.22 * (1.0 - s)

    modes = [(2, 1), (3, 1), (2, 2), (4, 1), (3, 2), (5, 1)]
    points, normals, shapes = [], [], []
    for r in range(rings + 1):
        s = r / rings
        radius, z = profile(s)
        ds = 1e-3
        r1, z1 = profile(min(s + ds, 1.0))
        r0, z0 = profile(max(s - ds, 0.0))
        dr, dz = r1 - r0, z1 - z0
        length = math.hypot(dr, dz)
        # Outward normal of the profile curve: rotate its tangent by -90 deg.
        nr, nz = dz / length, -dr / length
        if nr < 0:
            nr, nz = -nr, -nz
        for k in range(segments):
            theta = 2 * math.pi * k / segments
            c, sn = math.cos(theta), math.sin(theta)
            points += [radius * c, radius * sn, z]
            normals += [nr * c, nr * sn, nz]
            shapes += [math.cos(m * theta) * s ** 1.5 * math.cos((n - 1) * math.pi * s * 0.8)
                       for m, n in modes]
    triangles = []
    for r in range(rings):
        for k in range(segments):
            a, b = r * segments + k, r * segments + (k + 1) % segments
            c, d = a + segments, b + segments
            triangles += [a, b, d, a, d, c]
    freqs = [440.0 * f for f in (1.0, 2.4, 3.1, 4.2, 5.6, 6.3)]
    spec = {"object": "bell", "note": "analytic stand-in, not a solve",
            "modes": [list(mn) for mn in modes]}
    return freqs, points, normals, triangles, shapes, 1.2, spec


def main():
    if len(sys.argv) != 3 or sys.argv[1] not in ("plate", "bell"):
        sys.exit(__doc__)
    freqs, points, normals, triangles, shapes, mass, spec = {"plate": plate, "bell": bell}[sys.argv[1]]()
    rounded = lambda values: [round(v, 5) for v in values]
    data = {"format": "eseq-modes", "version": 1, "freqs": rounded(freqs),
            "points": rounded(points), "normals": rounded(normals), "triangles": triangles,
            "shapes": rounded(shapes), "mass": round(mass, 5), "spec": spec}
    with open(sys.argv[2], "w") as out:
        json.dump(data, out, separators=(",", ":"))
        out.write("\n")


if __name__ == "__main__":
    main()
