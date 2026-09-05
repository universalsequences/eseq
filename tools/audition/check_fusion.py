#!/usr/bin/env python3
"""Flag the DGenLisp fusion bug in a generated patch.c: a scalar history
write (memory[<literal>] = ...) emitted inside a tensor element loop
(for (int tNN = 0; tNN < N; ...)). See bd memory
dgen-scalar-cluster-fused-into-tensor-loop. Usage: check_fusion.py patch.c
"""
import re, sys
src = open(sys.argv[1]).read().splitlines()
bad = []
depth = 0
stack = []
for ln, line in enumerate(src, 1):
    m = re.search(r"for \(int (t\d+) = 0; \1 < (\d+);", line)
    if m:
        stack.append((ln, int(m.group(2)), line.count("{") - line.count("}")))
        continue
    if stack:
        stack[-1] = (stack[-1][0], stack[-1][1], stack[-1][2] + line.count("{") - line.count("}"))
        if re.search(r"memory\[\d+\] =", line):
            bad.append((ln, stack[-1][0], stack[-1][1], line.strip()))
        if stack[-1][2] <= 0:
            stack.pop()
for ln, loop_ln, n, text in bad:
    print(f"line {ln}: scalar history write inside {n}-element loop opened at line {loop_ln}: {text}")
print("FUSED" if bad else "clean", f"({len(bad)} scalar writes inside tensor loops)")
sys.exit(1 if bad else 0)
