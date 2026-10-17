import sys, os; sys.path.insert(0, os.path.dirname(__file__))
import sixop_patches as SP
from lut import array
b, i = int(sys.argv[1]), int(sys.argv[2])
d = array(f"syx_bank_{b}")[i*128:(i+1)*128]
p = SP.unpack(d)
for k, v in p.items():
    if k != "op": print(k, v)
for j, o in enumerate(p["op"]): print("op", 6 - j, o)
r = SP.row_for(p)
print({k: r[c] for k, c in SP.COLS.items() if not k.startswith("o")})
