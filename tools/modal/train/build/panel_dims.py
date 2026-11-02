"""Latent dims beyond the template panels' 4: controls for z5.. on the Latent Snare (A/B) and Latent Snare
Morph panels, which were laid out for a 4-number latent."""
import re


def tone_row(spec, voice):
    """The TONE row's knobs (comma-separated `spec`, only those the voice has a param for) and the knob
    width that keeps the row as wide as the template's 4 knobs at 5.6."""
    tone = [k for k in spec.split(",") if f"(param {k} " in voice]
    return tone, round(4 * 5.6 / max(4, len(tone)), 2)


def offsets(ui, D):
    """A/B panel: the OFFSET row gets one knob per dim, narrowed to fit the same block."""
    old = re.search(r"""\(each '\("z1" "z2" "z3" "z4"\) \|name\|\n( *)\(eseq\.effects\.custom-ui-lego/ui-lego-knob-full-s 1 name name 4\.8 """, ui)
    assert old, "OFFSET row"
    w = 4.8 if D <= 4 else round((24.0 - 4.9 - 0.1 * (D - 1)) / D, 2)
    names = " ".join(f'"z{k + 1}"' for k in range(D))
    return ui[:old.start()] + f"(each '({names}) |name|\n{old.group(1)}(eseq.effects.custom-ui-lego/ui-lego-knob-full-s 1 name name {w:g} " + ui[old.end():]


def depth(ui, D):
    """Morph panel: each START / END block gets small pickers for dims 5.., two per column."""
    extra = list(range(5, D + 1))
    if not extra:
        return ui
    cols = [extra[i:i + 2] for i in range(0, len(extra), 2)]
    tail = "(str side \"3\") (str side \"3\") 3.6 2 \"\" color false))"
    i = ui.index(tail, ui.index("(def lm-depth")) + len(tail)
    add = "".join("\n        (v-stack :width 3.6 :gap 0.20 :align :start" + "".join(
        f'\n          (eseq.effects.custom-ui-lego/ui-lego-micro-num-control-s section false (str side "{k}") (str side "{k}") 3.6 2 "" color false)'
        for k in col) + ")" for col in cols)
    ui = ui[:i] + add + ui[i:]
    old = "(def lm-side-w () 16.4)"
    assert old in ui
    return ui.replace(old, f"(def lm-side-w () {16.4 + 3.9 * len(cols):g})")
