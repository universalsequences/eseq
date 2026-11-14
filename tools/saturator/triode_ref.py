"""Reference model of the Saturator "triode" curve: one 12AX7 common-cathode stage.

Circuit (values in CIRCUIT):

    e(t) --Rs--||Cin--+--Rgs-- G          Vb
                      |                    |
                      Rg                   Rp
                      |                    |
                    Vbias          P ------+--||Cout--+-- out
                                           |          RL
              K --+--Rk-- gnd              Cl         |
                  +--Ck-- gnd              gnd       gnd

Cl is the capacitance the plate drives (the next stage's Miller capacitance
plus strays). It is what rounds the edges of a clipped wave in the real
circuit, and here it also keeps those edges from aliasing.

Tube: Dempwolf & Zoelzer, "A physically-motivated triode model for circuit
simulations" (DAFx-11), 12AX7 fit, including grid current:

    Ik = G  * (log(1 + exp(C  * (Vak/mu + Vgk))) / C )**gamma
    Ig = Gg * (log(1 + exp(Cg * Vgk)) / Cg)**xi + Ig0
    Ia = Ik - Ig

Two independent solvers of the same circuit live here:
  * `solve_reference`: stiff ODE integration (scipy Radau, tight tolerances).
  * `TriodeStage`: the discrete algorithm the Rust engine implements
    (trapezoidal companion models + 2x2 Newton on Vgk, Vak).
`compare.py` checks the second against the first and writes the fixtures the
Rust tests assert against.
"""
import numpy as np
from scipy.integrate import solve_ivp
from scipy.optimize import fsolve

TUBE = dict(G=2.242e-3, C=3.40, mu=103.2, gamma=1.26,
            Gg=6.177e-4, Cg=9.901, xi=1.314, Ig0=8.025e-8)

CIRCUIT = dict(Vb=300.0, Rp=100e3, Rk=1.5e3, Ck=22e-6, Rs=38e3, Cin=22e-9,
               Rg=1e6, Rgs=68e3, Cout=22e-9, RL=1e6, Cl=100e-12)


def softplus(x):
    return np.logaddexp(0.0, x)


def tube_currents(vgk, vak, t=TUBE):
    ik = t["G"] * (softplus(t["C"] * (vak / t["mu"] + vgk)) / t["C"]) ** t["gamma"]
    ig = t["Gg"] * (softplus(t["Cg"] * vgk) / t["Cg"]) ** t["xi"] + t["Ig0"]
    return ik, ig


def node_voltages(e, vbias, vcin, vck, vcout, vp, c=CIRCUIT):
    """Solve the memoryless part given the capacitor voltages.

    vcin = V(A) - V(B) across Cin, vck = V(K), vcout = V(P) - V(out), vp = V(P)
    (across Cl). Returns (vg, vk, vp, vout, i_cin, i_ck, i_cout, i_cl).
    """
    vk = vck

    def resid(u):
        vg = u[0]
        _, ig = tube_currents(vg - vk, vp - vk)
        # grid branch: i_cin flows e -> Rs -> Cin -> B
        vb_node = vg + ig * c["Rgs"]
        i_cin = (e - vcin - vb_node) / c["Rs"]
        return [(i_cin - (vb_node - vbias) / c["Rg"] - ig) * 1e3]

    vg = fsolve(resid, [node_voltages.guess], xtol=1e-13)[0]
    node_voltages.guess = vg
    ik, ig = tube_currents(vg - vk, vp - vk)
    vb_node = vg + ig * c["Rgs"]
    i_cin = (e - vcin - vb_node) / c["Rs"]
    vout = vp - vcout
    i_cout = vout / c["RL"]
    i_ck = ik - vk / c["Rk"]
    i_cl = (c["Vb"] - vp) / c["Rp"] - (ik - ig) - i_cout
    return vg, vk, vp, vout, i_cin, i_ck, i_cout, i_cl


node_voltages.guess = 0.0


def operating_point(vbias=0.0, c=CIRCUIT):
    """DC solution: caps open."""
    def resid(u):
        vg, vk, vp = u
        ik, ig = tube_currents(vg - vk, vp - vk)
        return [((vbias - vg) / (c["Rg"] + c["Rgs"]) - ig) * 1e6,
                (ik - vk / c["Rk"]) * 1e3,
                ((c["Vb"] - vp) / c["Rp"] - (ik - ig)) * 1e3]
    vg, vk, vp = fsolve(resid, [vbias, 1.5, 200.0], xtol=1e-13)
    ik, ig = tube_currents(vg - vk, vp - vk)
    # cap voltages at rest (e = 0): no current in Rs, V(A)=0, V(B)=vg+ig*Rgs
    return dict(vg=vg, vk=vk, vp=vp, vcin=0.0 - (vg + ig * c["Rgs"]), vck=vk, vcout=vp)


def solve_reference(e_fn, t_end, fs_out, vbias=0.0, c=CIRCUIT, rtol=1e-9, atol=1e-11,
                    start_vbias=None):
    """`start_vbias`: bias the circuit is at rest with at t=0 (default: `vbias`).
    The engine always starts at rest with 0, so its fixtures pass 0."""
    op = operating_point(vbias if start_vbias is None else start_vbias, c)
    node_voltages.guess = op["vg"]

    def rhs(t, y):
        _, _, _, _, i_cin, i_ck, i_cout, i_cl = node_voltages(e_fn(t), vbias, *y, c)
        return [i_cin / c["Cin"], i_ck / c["Ck"], i_cout / c["Cout"], i_cl / c["Cl"]]

    n = int(round(t_end * fs_out))
    t_eval = np.arange(n) / fs_out
    sol = solve_ivp(rhs, (0.0, t_eval[-1]), [op["vcin"], op["vck"], op["vcout"], op["vp"]],
                    method="Radau", t_eval=t_eval, rtol=rtol, atol=atol,
                    max_step=0.25 / fs_out)
    out = sol.y[3] - sol.y[2]
    return t_eval, out, sol.y


class TriodeStage:
    """Discrete twin of crates/sequencer/src/effects/triode.rs (keep in sync)."""

    def __init__(self, fs, vbias=0.0, c=CIRCUIT, t=TUBE, iters=12, start_vbias=None):
        self.c, self.t, self.iters = c, t, iters
        self.vbias = vbias
        T = 1.0 / fs
        self.gcin = 2 * c["Cin"] / T
        self.gck = 2 * c["Ck"] / T
        self.gcout = 2 * c["Cout"] / T
        self.gcl = 2 * c["Cl"] / T
        op = operating_point(vbias if start_vbias is None else start_vbias, c)
        self.vgk, self.vak = op["vg"] - op["vk"], op["vp"] - op["vk"]
        # trapezoidal cap states: (voltage, current)
        self.vcin, self.icin = op["vcin"], 0.0
        self.vck, self.ick = op["vck"], 0.0
        self.vcout, self.icout = op["vcout"], 0.0
        self.vcl, self.icl = op["vp"], 0.0
        # constant Thevenin resistances
        ra = c["Rs"] + 1.0 / self.gcin           # e -> B series arm
        self.rb = ra * c["Rg"] / (ra + c["Rg"])  # B node Thevenin R
        self.ka = c["Rg"] / (ra + c["Rg"])       # weight of the e arm
        self.kb = ra / (ra + c["Rg"])            # weight of the Vbias arm
        self.rgth = self.rb + c["Rgs"]
        self.zk = 1.0 / (1.0 / c["Rk"] + self.gck)
        self.rl = c["RL"] + 1.0 / self.gcout     # P -> gnd load arm
        self.rpth = 1.0 / (1.0 / c["Rp"] + 1.0 / self.rl + self.gcl)

    def step(self, e):
        c, t = self.c, self.t
        # companion sources: v_C[n] = i_C[n]/g + (v_C[n-1] + i_C[n-1]/g)
        hin = self.vcin + self.icin / self.gcin
        hk = self.vck + self.ick / self.gck
        hout = self.vcout + self.icout / self.gcout
        eg = self.ka * (e - hin) + self.kb * self.vbias
        hcl = self.gcl * self.vcl + self.icl
        ep = self.rpth * (c["Vb"] / c["Rp"] + hout / self.rl + hcl)
        vk0 = self.zk * self.gck * hk
        vgk, vak = self.vgk, self.vak
        for _ in range(self.iters):
            sg = t["C"] * (vak / t["mu"] + vgk)
            lg = np.logaddexp(0.0, sg) / t["C"]
            sig = 1.0 / (1.0 + np.exp(-sg))
            ik = t["G"] * lg ** t["gamma"]
            dik = t["G"] * t["gamma"] * lg ** (t["gamma"] - 1.0) * sig  # d/dvgk
            dik_a = dik / t["mu"]
            sq = t["Cg"] * vgk
            lq = np.logaddexp(0.0, sq) / t["Cg"]
            ig = t["Gg"] * lq ** t["xi"] + t["Ig0"]
            dig = t["Gg"] * t["xi"] * lq ** (t["xi"] - 1.0) / (1.0 + np.exp(-sq))
            ia = ik - ig
            f1 = eg - self.rgth * ig - self.zk * ik - vk0 - vgk
            f2 = ep - self.rpth * ia - self.zk * ik - vk0 - vak
            a11 = -self.rgth * dig - self.zk * dik - 1.0
            a12 = -self.zk * dik_a
            a21 = -self.rpth * (dik - dig) - self.zk * dik
            a22 = -self.rpth * dik_a - self.zk * dik_a - 1.0
            det = a11 * a22 - a12 * a21
            dv1 = (f1 * a22 - f2 * a12) / det
            dv2 = (a11 * f2 - a21 * f1) / det
            # Step limiting (as in triode.rs): free in cutoff, where the
            # circuit is linear, but never far up into conduction at once.
            vgk = min(vgk - dv1, max(vgk, -3.0) + 0.5)
            vak = min(max(vak - dv2, 0.0), 2.0 * c["Vb"])
        ik, ig = tube_currents(vgk, vak, t)
        ia = ik - ig
        self.vgk, self.vak = vgk, vak
        vk = vk0 + self.zk * ik
        vp = vak + vk
        vg = vgk + vk
        # update cap states
        vb_node = vg + ig * c["Rgs"]
        icin = (vb_node - self.vbias) / c["Rg"] + ig
        self.vcin, self.icin = hin + icin / self.gcin, icin
        ick = ik - vk / c["Rk"]
        self.vck, self.ick = vk, ick
        icout = (vp - hout) / self.rl
        self.vcout, self.icout = hout + icout / self.gcout, icout
        self.vcl, self.icl = vp, self.gcl * vp - hcl
        return vp - self.vcout

    def run(self, e):
        return np.array([self.step(x) for x in e])


if __name__ == "__main__":
    op = operating_point()
    print("operating point:", {k: round(float(v), 4) for k, v in op.items()})
    ik, ig = tube_currents(op["vg"] - op["vk"], op["vp"] - op["vk"])
    print("Ia mA", (ik - ig) * 1e3, "Ig uA", ig * 1e6)
    fs = 192000
    st = TriodeStage(fs)
    n = fs // 10
    t = np.arange(n) / fs
    for amp in (0.01, 0.5, 1.0, 2.0, 5.0, 20.0):
        st = TriodeStage(fs)
        y = st.run(amp * np.sin(2 * np.pi * 1000 * t))
        seg = y[-fs // 100:]
        print(f"amp {amp:6.2f} V: out min {seg.min():8.2f} max {seg.max():8.2f}  gain(pk-pk) {(seg.max()-seg.min())/(2*amp):7.2f}")
