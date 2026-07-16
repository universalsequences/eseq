#!/usr/bin/env python3
"""Scan samples/ directory: fix filenames and remove files hound can't load.

Phase 1: Fix double .wav.wav extensions
Phase 2: Strip control/invisible characters from filenames
Phase 3: Remove files with headers hound can't parse
"""

import os
import struct
import sys
import unicodedata

SAMPLES_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "samples")


def check_wav_loadable(path):
    """Check if hound can load this WAV. Mirrors hound's read.rs validation."""
    try:
        with open(path, "rb") as f:
            header = f.read(12)
            if len(header) < 12:
                return "file too small"
            if header[0:4] != b"RIFF":
                return f"not a RIFF file (got {header[0:4]!r})"
            if header[8:12] != b"WAVE":
                return f"not a WAVE file (got {header[8:12]!r})"

            file_size = os.fstat(f.fileno()).st_size
            found_fmt = False
            found_data = False
            bytes_per_sample = 0

            while True:
                chunk_header = f.read(8)
                if len(chunk_header) < 8:
                    break

                chunk_id = chunk_header[0:4]
                chunk_size = struct.unpack_from("<I", chunk_header, 4)[0]
                chunk_start = f.tell()

                if chunk_id == b"fmt ":
                    if chunk_size < 16:
                        return "invalid fmt chunk size"
                    fmt_data = f.read(min(chunk_size, 40))
                    if len(fmt_data) < 16:
                        return "truncated fmt chunk"

                    format_tag = struct.unpack_from("<H", fmt_data, 0)[0]
                    n_channels = struct.unpack_from("<H", fmt_data, 2)[0]
                    n_samples_per_sec = struct.unpack_from("<I", fmt_data, 4)[0]
                    n_bytes_per_sec = struct.unpack_from("<I", fmt_data, 8)[0]
                    block_align = struct.unpack_from("<H", fmt_data, 12)[0]
                    bits_per_sample = struct.unpack_from("<H", fmt_data, 14)[0]

                    # hound checks
                    if n_channels == 0:
                        return "zero channels"

                    bps = block_align // n_channels if n_channels > 0 else 0
                    if bits_per_sample > bps * 8:
                        return "sample bits exceeds size of sample"

                    if n_bytes_per_sec != block_align * n_samples_per_sec:
                        return "inconsistent fmt chunk"

                    if bits_per_sample % 8 != 0:
                        return "bits per sample not multiple of 8"

                    if bits_per_sample == 0:
                        return "bits per sample is 0"

                    if format_tag not in (1, 3, 0xFFFE):
                        if format_tag == 2:
                            return "ADPCM unsupported"
                        return f"unsupported format tag ({format_tag})"

                    # Format-specific chunk size checks (from hound)
                    if format_tag == 1:  # PCM
                        if chunk_size not in (16, 18, 40):
                            return f"unexpected fmt chunk size ({chunk_size}) for PCM"
                    elif format_tag == 3:  # IEEE float
                        if chunk_size not in (16, 18):
                            return f"unexpected fmt chunk size ({chunk_size}) for float"
                    elif format_tag == 0xFFFE:  # Extensible
                        if chunk_size < 40:
                            return f"fmt chunk too small for extensible ({chunk_size})"

                    bytes_per_sample = block_align
                    found_fmt = True

                    # Seek to end of chunk
                    f.seek(chunk_start + chunk_size)

                elif chunk_id == b"data":
                    if not found_fmt:
                        return "data chunk before fmt"
                    # hound checks: data_len must be multiple of bytes_per_sample
                    if bytes_per_sample > 0 and chunk_size % bytes_per_sample != 0:
                        return f"data length not multiple of sample size"
                    found_data = True
                    break

                else:
                    if chunk_size > file_size:
                        return "chunk size exceeds file"
                    f.seek(chunk_start + chunk_size)

                # WAV chunks are word-aligned
                if chunk_size % 2 != 0:
                    f.seek(1, 1)

            if not found_fmt:
                return "no fmt chunk"
            if not found_data:
                return "no data chunk"

    except Exception as e:
        return str(e)

    return None


def sanitize_filename(name):
    """Remove control characters and other invisible chars from a filename."""
    cleaned = []
    for ch in name:
        cat = unicodedata.category(ch)
        if cat.startswith("C"):
            continue
        cleaned.append(ch)
    result = "".join(cleaned).strip()
    while "  " in result:
        result = result.replace("  ", " ")
    return result


def fix_double_extensions(samples_dir, dry_run=False):
    """Rename files with .wav.wav or .wav_N.wav to single .wav extension."""
    renamed = 0
    for root, _, files in os.walk(samples_dir):
        for fname in files:
            if not fname.lower().endswith(".wav"):
                continue
            base = fname[:-4]
            if base.lower().endswith(".wav") or ".wav_" in base.lower():
                if base.lower().endswith(".wav"):
                    new_name = base
                else:
                    idx = base.lower().rfind(".wav_")
                    new_name = base[:idx] + base[idx + 4 :] + ".wav"

                old_path = os.path.join(root, fname)
                new_path = os.path.join(root, new_name)
                if os.path.exists(new_path) and old_path != new_path:
                    stem = new_name[:-4]
                    n = 1
                    while os.path.exists(new_path):
                        new_name = f"{stem}_{n}.wav"
                        new_path = os.path.join(root, new_name)
                        n += 1
                if old_path != new_path:
                    if not dry_run:
                        os.rename(old_path, new_path)
                    renamed += 1
    return renamed


def fix_control_chars(samples_dir, dry_run=False):
    """Rename files/dirs that contain control or invisible characters."""
    fixed = 0
    for root, dirs, files in os.walk(samples_dir, topdown=False):
        for fname in files:
            clean = sanitize_filename(fname)
            if clean != fname and clean:
                old_path = os.path.join(root, fname)
                new_path = os.path.join(root, clean)
                if os.path.exists(new_path) and old_path != new_path:
                    stem = clean[:-4] if clean.lower().endswith(".wav") else clean
                    ext = ".wav" if clean.lower().endswith(".wav") else ""
                    n = 1
                    while os.path.exists(new_path):
                        new_path = os.path.join(root, f"{stem}_{n}{ext}")
                        n += 1
                if not dry_run:
                    os.rename(old_path, new_path)
                fixed += 1
            elif not clean:
                old_path = os.path.join(root, fname)
                if not dry_run:
                    os.remove(old_path)
                fixed += 1

        for dname in dirs:
            clean = sanitize_filename(dname)
            if clean != dname and clean:
                old_path = os.path.join(root, dname)
                new_path = os.path.join(root, clean)
                if os.path.exists(new_path) and old_path != new_path:
                    n = 1
                    while os.path.exists(new_path):
                        new_path = os.path.join(root, f"{clean}_{n}")
                        n += 1
                if not dry_run:
                    os.rename(old_path, new_path)
                fixed += 1
    return fixed


def main():
    dry_run = "--dry-run" in sys.argv

    if not os.path.isdir(SAMPLES_DIR):
        print(f"samples directory not found: {SAMPLES_DIR}")
        sys.exit(1)

    # Phase 1: Fix double extensions
    ext_count = fix_double_extensions(SAMPLES_DIR, dry_run)
    if ext_count:
        verb = "Would rename" if dry_run else "Renamed"
        print(f"{verb} {ext_count} files with double .wav extensions.")

    # Phase 2: Fix control characters in filenames
    ctrl_count = fix_control_chars(SAMPLES_DIR, dry_run)
    if ctrl_count:
        verb = "Would fix" if dry_run else "Fixed"
        print(f"{verb} {ctrl_count} files/dirs with control characters.")

    if ext_count or ctrl_count:
        print()

    # Phase 3: Find files hound can't load
    bad_files = []
    good_count = 0

    for root, _, files in os.walk(SAMPLES_DIR):
        for fname in sorted(files):
            if not fname.lower().endswith(".wav"):
                continue
            path = os.path.join(root, fname)
            err = check_wav_loadable(path)
            if err:
                rel = os.path.relpath(path, SAMPLES_DIR)
                bad_files.append((path, rel, err))
            else:
                good_count += 1

    print(f"Scanned: {good_count + len(bad_files)} files, {good_count} OK, {len(bad_files)} bad\n")

    if not bad_files:
        print("No bad files to remove.")
        return

    for _, rel, err in bad_files:
        print(f"  BAD: {rel} — {err}")

    if dry_run:
        print(
            f"\nDry run: would delete {len(bad_files)} files. Run without --dry-run to delete."
        )
    else:
        print()
        for path, rel, _ in bad_files:
            os.remove(path)
            print(f"  Deleted: {rel}")

        for root, _, _ in os.walk(SAMPLES_DIR, topdown=False):
            if root != SAMPLES_DIR and not os.listdir(root):
                os.rmdir(root)
                print(f"  Removed empty dir: {os.path.relpath(root, SAMPLES_DIR)}")

        print(f"\nDone. Removed {len(bad_files)} bad files.")


if __name__ == "__main__":
    main()
