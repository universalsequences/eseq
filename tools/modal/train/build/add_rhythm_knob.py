"""Add knobs after `level` in a staged panel's TONE row (rhythm, and metal when the voice has it), resizing
the row so it keeps the width of the original 4 knobs at 5.6. usage: add_rhythm_knob.py UI_LISP VOICE_LISP [KNOBS]
KNOBS: comma-separated (default rhythm,metal); only the ones the voice has a param for are added."""
import re, sys
p, voice = sys.argv[1], open(sys.argv[2]).read()
want = sys.argv[3].split(",") if len(sys.argv) > 3 else ["rhythm", "metal"]
extra = [k for k in want if f"(param {k} " in voice]
if not extra:
    sys.exit(0)
s = open(p).read()
w = round(4 * 5.6 / (4 + len(extra)), 2)
pat = re.compile(r'(\(eseq\.effects\.custom-ui-lego/ui-lego-knob-full-s (\d) "(tune|decay|snappy|level)" "\3" )5\.6( \((?:ls|lm)-accent\) 2\))')
assert len(pat.findall(s)) == 4, pat.findall(s)
s = pat.sub(lambda m: f"{m.group(1)}{w}{m.group(4)}", s)
m = re.search(r'\(eseq\.effects\.custom-ui-lego/ui-lego-knob-full-s (\d) "level" "level" [\d.]+ \(((?:ls|lm)-accent)\) 2\)', s)
s = s[:m.end()] + "".join(f'\n        (eseq.effects.custom-ui-lego/ui-lego-knob-full-s {m.group(1)} "{k}" "{k}" {w} ({m.group(2)}) 2)' for k in extra) + s[m.end():]
open(p, "w").write(s); print(", ".join(extra), "knobs ->", p)
