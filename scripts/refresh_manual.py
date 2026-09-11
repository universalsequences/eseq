#!/usr/bin/env python3
"""Rebuild manual images from the current app, then export the manual to HTML."""

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import subprocess
import sys
import tempfile

from capture_manual_images import ROOT, build_binary, capture_image, load_captures


@dataclass(frozen=True)
class ImageRecipe:
    reference: str
    capture: dict = None
    diagram: Path = None


def discover_images(exporter, source):
    result = subprocess.run(
        [str(exporter), "--source", str(source), "--list-images"],
        cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
    return json.loads(result.stdout)


def plan_images(source, references, captures):
    """Require exactly one generation source for every referenced PNG."""
    recipes = []
    capture_outputs = {f"images/{name}.png": entry for name, entry in captures.items()}
    for reference in sorted(set(references)):
        path = PurePosixPath(reference)
        if (not reference.startswith("images/") or path.suffix != ".png"
                or any(char in reference for char in ":\\?#")
                or any(part in ("", ".", "..") for part in reference.split("/"))):
            raise ValueError(f"expected a local images/*.png reference: {reference!r}")
        capture = capture_outputs.get(reference)
        diagram = source / "diagrams" / path.relative_to("images").with_suffix(".svg")
        if capture is not None and diagram.is_file():
            raise ValueError(f"{reference}: both a capture recipe and {diagram} exist; choose one")
        if capture is None and not diagram.is_file():
            raise ValueError(
                f"{reference}: no generation source; add a recipe to "
                f"crates/sequencer/ui/capture-fixtures/manual-images.json or author {diagram}")
        recipes.append(ImageRecipe(reference, capture, diagram if capture is None else None))
    return recipes


def install_image(staged, destination):
    """Replace one PNG without exposing a partly copied file to the app."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=destination.parent, prefix=".manual-", delete=False) as file:
        temporary = Path(file.name)
    try:
        shutil.copy2(staged, temporary)
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def refresh(source, output, *, release=True):
    source, output = source.resolve(), output.resolve()
    if output == source or source in output.parents:
        raise ValueError("HTML output must be outside the manual source directory")
    captures = load_captures()
    exporter = build_binary("eseqlisp", "eseqlisp_manual_export", release=release)
    with tempfile.TemporaryDirectory(prefix="eseq-manual-") as temporary:
        staged = Path(temporary) / "manual"
        staged.mkdir()
        # A consistent snapshot of the prose; existing PNGs are never reused.
        for page in source.glob("*.md"):
            shutil.copy2(page, staged / page.name)
        recipes = plan_images(source, discover_images(exporter, staged), captures)
        renderer = shutil.which("rsvg-convert")
        if any(recipe.diagram for recipe in recipes) and renderer is None:
            raise RuntimeError("SVG figures require librsvg; install it with: brew install librsvg")
        binary = None
        if any(recipe.capture is not None for recipe in recipes):
            binary = build_binary("sequencer", "metal_seq", release=release)
        for index, recipe in enumerate(recipes, 1):
            destination = staged / recipe.reference
            destination.parent.mkdir(parents=True, exist_ok=True)
            if recipe.capture is not None:
                print(f"[{index}/{len(recipes)}] Capturing {recipe.reference}", flush=True)
                capture_image(binary, recipe.capture, destination)
            else:
                print(f"[{index}/{len(recipes)}] Rendering {recipe.reference}", flush=True)
                subprocess.run([renderer, "--zoom", "2", "--output", str(destination),
                                str(recipe.diagram)], cwd=ROOT, check=True)

        # Export validates pages, links, PNGs, and output ownership before writing.
        # A generation/validation failure leaves both existing readers untouched.
        print(f"Exporting HTML to {output}", flush=True)
        subprocess.run([str(exporter), "--source", str(staged), "--out", str(output)],
                       cwd=ROOT, check=True)
        for recipe in recipes:
            install_image(staged / recipe.reference, source / recipe.reference)
    print(f"Updated {len(recipes)} images in {source / 'images'}", flush=True)
    print(f"Manual ready: {output / 'index.html'}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=ROOT.parent / "eseq-site/manual",
                        help="HTML destination (default: ../eseq-site/manual beside this repo)")
    parser.add_argument("--dev", action="store_true",
                        help="Build with Cargo's dev profile instead of release")
    args = parser.parse_args()
    if platform.system() != "Darwin":
        parser.error("real UI capture requires macOS and Metal")
    refresh(ROOT / "docs/manual", args.out, release=not args.dev)


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(f"manual refresh: {error}")
