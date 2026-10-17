"""Exact stmlib::Random::GetFloat() sequence (rng_state_ = 0x21) as a
[N 1] tensor json, so random engines can be fed the reference's draws."""
import json, sys, numpy as np
def draws(n, state=0x21):
    out = np.empty(n, dtype=np.float32)
    for i in range(n):
        state = (state * 1664525 + 1013904223) & 0xFFFFFFFF
        out[i] = np.float32(state) / np.float32(4294967296.0)
    return out
if __name__ == "__main__":
    n, path = int(sys.argv[1]), sys.argv[2]
    d = draws(n)
    json.dump({"shape": [n, 1], "data": [float(x) for x in d]}, open(path, "w"), separators=(",", ":"))
