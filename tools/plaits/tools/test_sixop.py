import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import plh

t = plh.EngineTest("sixop", ["engines/sixop.lisp"],
    "(plaits-sixop bstart bphase rise high patched note timbre morph harmonics accent p_bank)",
    data_files=["data/sixop_patches.json", "data/sixop_cube_root.json"],
    extra_params=["(param p_bank @default 0 @min 0 @max 2)"])


def cmp(label, bank=0, **kw):
    return t.compare(label, port={"p_bank": bank}, engine=2 + bank, **kw)


if __name__ == "__main__":
    which = sys.argv[1] if len(sys.argv) > 1 else "all"
    if which in ("quick",):
        cmp("b0 h0.5 gate", bank=0, harmonics=0.5, seconds=1.0, gate=0.5)
        sys.exit()
    for bank in (0, 1, 2):
        for h, tb, m in [(0.0, 0.5, 0.5), (0.5, 0.5, 0.5), (1.0, 0.0, 0.0), (0.3, 1.0, 1.0)]:
            cmp(f"b{bank} h{h} t{tb} m{m}", bank=bank, harmonics=h, timbre=tb, morph=m, gate=0.5, seconds=1.5)
    cmp("b1 retrig .25", bank=1, harmonics=0.2, gate=0.1531, retrig=0.25037, seconds=2)
    cmp("b2 retrig .25", bank=2, harmonics=0.7, gate=0.1531, retrig=0.25037, seconds=2)
    cmp("b0 note sweep", bank=0, harmonics=0.4, note=36, note_end=84, gate=1.8, seconds=2)
    cmp("b1 h sweep retrig", bank=1, harmonics=0, harmonics_end=1, gate=0.1031, retrig=0.20037, seconds=3)
    cmp("b0 h sweep retrig", bank=0, harmonics=0, harmonics_end=1, gate=0.1031, retrig=0.20037, seconds=3)
    cmp("b2 h sweep retrig", bank=2, harmonics=1, harmonics_end=0, gate=0.1031, retrig=0.20037, seconds=3)
    cmp("b0 t/m sweep retrig", bank=0, harmonics=0.65, timbre=0, timbre_end=1, morph=1, morph_end=0, gate=0.3031, retrig=0.40037, seconds=3)
    cmp("b1 accent .2 note 72", bank=1, harmonics=0.55, accent=0.2, note=72, gate=0.7031, seconds=1.5)
    cmp("b2 accent 1 note 30", bank=2, harmonics=0.85, accent=1.0, note=30, gate=0.7031, seconds=1.5)
    cmp("b0 unpatched m.3", bank=0, trig=0, harmonics=0.5, morph=0.3, seconds=1.5)
    cmp("b2 unpatched m.8", bank=2, trig=0, harmonics=0.3, morph=0.8, timbre=0.7, seconds=1.5)
    cmp("b1 unpatched msweep", bank=1, trig=0, harmonics=0.6, morph=0, morph_end=1, seconds=2)
    t.cpu(seconds=5, harmonics=0.5, gate=0.5, retrig=0.25)
