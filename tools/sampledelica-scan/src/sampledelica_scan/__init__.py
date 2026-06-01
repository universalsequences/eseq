"""Sampledelica scanner — prototype harmonic miner for an album library.

This package is intentionally decoupled from the Rust crates. It analyzes audio
and writes *sidecar* JSON + rendered slice WAVs + spectrograms to an output dir.
When the detection results are good, a separate Rust "bridge" reads those
sidecars into the sequencer's SQLite Tier-2 tables. Detection and storage never
get coupled.
"""

__version__ = "0.1.0"
