"""Generate the test-only Lisp that feeds the reference LCG draws to
particle-pre in the exact order the C++ consumes them (particle p of
block b starts at o_p, uses 13 draws + 1 if it fired in the block)."""
def gen(table="particle_lcg"):
    L = []
    L.append("(def pt_ds (pl-note-to-freq (+ 60 (* timbre timbre 72))))")
    L.append("(def pt_den (/ (* pt_ds pt_ds) 6))")
    L.append("(make-history pt_h_base)")
    L.append("(def pt_o0 (read-history pt_h_base))")
    args = []
    for p in range(6):
        o = f"pt_o{p}"
        nested = "12"
        for i in reversed(range(12)):
            nested = f"(gswitch (lte (peek {table} (+ {o} {i}) 0) pt_den) {i} {nested})"
        L.append(f"(def pt_k{p} (gswitch rise 0 {nested}))")
        L.append(f"(def pt_o{p+1} (+ {o} 13 (lt pt_k{p} 12)))")
        L.append(f"(def pt_u{p} (peek {table} (+ {o} bphase (gt bphase pt_k{p})) 0))")
        L.append(f"(def pt_uf{p} (peek {table} (+ {o} pt_k{p} 1) 0))")
        args += [f"pt_u{p}", f"pt_uf{p}"]
    L.append("(write-history pt_h_base (gswitch (eq bphase 11) pt_o6 pt_o0))")
    call = "(particle-pre bstart rise note timbre morph harmonics " + " ".join(args) + ")"
    return "\n".join(L) + "\n", call
