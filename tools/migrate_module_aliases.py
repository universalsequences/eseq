#!/usr/bin/env python3
"""Extract, audit, and rewrite the module compatibility-alias vocabulary.

The checked-in TSV is intentionally independent of the Lisp alias forms: it can
remain as a user-content migration dictionary after the compatibility table is
removed. Rewrites are token-aware. They never replace substrings, string
contents, or quoted data; those occurrences are reported for manual triage.
"""

from __future__ import annotations

import argparse
import csv
import dataclasses
import pathlib
import re
import subprocess
import sys
from collections.abc import Iterable

ROOT = pathlib.Path(__file__).resolve().parents[1]
UI_ROOT = ROOT / "content/ui"
DEFAULT_TABLE = ROOT / "tools/module-compat-aliases.tsv"
MODULE_RE = re.compile(r"\(module\s+([^\s()]+)\)")
ALIAS_RE = re.compile(r"\(module-compat-alias\s+([^\s()]+)\s+([^\s()]+)\)")
DELIMITERS = frozenset("()[]{}\"'`,;")
CONTENT_PREFIXES = (
    "content/instruments/",
    "content/effects/",
    "content/defmacros/",
    "content/midi-fx/",
    "content/scripts/",
)
UI_CONTENT_PREFIXES = (
    "crates/sequencer/ui/capture-fixtures/",
    "content/ui/themes/",
)
CATEGORIES = ("instruments", "effects", "defmacros", "midi-fx", "scripts", "fixtures+themes")


@dataclasses.dataclass(frozen=True)
class Alias:
    old: str
    new: str
    source: str


@dataclasses.dataclass(frozen=True)
class Token:
    kind: str
    start: int
    end: int
    text: str
    quoted: bool = False


def extract_aliases() -> list[Alias]:
    found: dict[str, Alias] = {}
    conflicts: list[tuple[Alias, Alias]] = []
    for path in sorted(UI_ROOT.rglob("*.lisp")):
        source = path.read_text()
        matches = list(ALIAS_RE.finditer(source))
        if not matches:
            continue
        module_match = MODULE_RE.search(source)
        if module_match is None:
            raise RuntimeError(f"{path.relative_to(ROOT)} has aliases but no module declaration")
        module = module_match.group(1)
        for match in matches:
            old, relative_new = match.groups()
            new = relative_new if "/" in relative_new else f"{module}/{relative_new}"
            alias = Alias(old, new, path.relative_to(ROOT).as_posix())
            previous = found.get(old)
            if previous is not None and previous.new != new:
                conflicts.append((previous, alias))
            else:
                found[old] = alias
    if conflicts:
        details = "\n".join(
            f"  {a.old}: {a.new} ({a.source}) != {b.new} ({b.source})" for a, b in conflicts
        )
        raise RuntimeError(f"compat aliases have conflicting targets:\n{details}")
    return sorted(found.values(), key=lambda alias: alias.old)


def write_table(path: pathlib.Path, aliases: Iterable[Alias]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="") as handle:
        writer = csv.writer(handle, delimiter="\t", lineterminator="\n")
        writer.writerow(("old", "new", "source"))
        for alias in aliases:
            writer.writerow((alias.old, alias.new, alias.source))


def load_table(path: pathlib.Path) -> dict[str, Alias]:
    with path.open(newline="") as handle:
        rows = csv.DictReader(handle, delimiter="\t")
        aliases = {row["old"]: Alias(row["old"], row["new"], row["source"]) for row in rows}
    if not aliases:
        raise RuntimeError(f"empty alias table: {path}")
    return aliases


def tracked_lisp_files() -> list[pathlib.Path]:
    result = subprocess.run(
        ["git", "ls-files", "-z", "--", "*.lisp"], cwd=ROOT, check=True, capture_output=True
    )
    return [ROOT / value.decode() for value in result.stdout.split(b"\0") if value]


def category(path: pathlib.Path) -> str | None:
    relative = path.relative_to(ROOT).as_posix()
    if path.name == "dsp.lisp":
        return None
    for name in CATEGORIES[:-1]:
        if relative.startswith(f"content/{name}/"):
            return name
    if relative == "content/ui/themes.lisp" or relative.startswith(UI_CONTENT_PREFIXES):
        return "fixtures+themes"
    return None


def read_excludes(paths: list[str], exclude_file: pathlib.Path | None) -> tuple[str, ...]:
    values = list(paths)
    if exclude_file is not None:
        values.extend(
            line.strip() for line in exclude_file.read_text().splitlines()
            if line.strip() and not line.lstrip().startswith("#")
        )
    return tuple(value.rstrip("/") for value in values)


def in_excluded_path(path: pathlib.Path, excludes: tuple[str, ...]) -> bool:
    relative = path.relative_to(ROOT).as_posix()
    return any(relative == value or relative.startswith(value + "/") for value in excludes)


def content_files(categories: set[str], excludes: tuple[str, ...]) -> list[pathlib.Path]:
    return [
        path for path in tracked_lisp_files()
        if category(path) in categories and not in_excluded_path(path, excludes)
    ]


def raw_tokens(source: str) -> list[Token]:
    tokens: list[Token] = []
    index = 0
    while index < len(source):
        char = source[index]
        if char.isspace():
            index += 1
            continue
        if char == ";":
            end = source.find("\n", index)
            if end < 0:
                end = len(source)
            tokens.append(Token("comment", index, end, source[index:end]))
            index = end
            continue
        if char == '"':
            end = index + 1
            escaped = False
            while end < len(source):
                current = source[end]
                if escaped:
                    escaped = False
                elif current == "\\":
                    escaped = True
                elif current == '"':
                    end += 1
                    break
                end += 1
            else:
                raise RuntimeError("unterminated string")
            tokens.append(Token("string", index, end, source[index:end]))
            index = end
            continue
        kinds = {"(": "open", ")": "close", "'": "quote", "`": "quasiquote", ",": "unquote"}
        if char in kinds:
            tokens.append(Token(kinds[char], index, index + 1, char))
            index += 1
            continue
        if char in "[]{}":
            tokens.append(Token("delimiter", index, index + 1, char))
            index += 1
            continue
        end = index + 1
        while end < len(source) and not source[end].isspace() and source[end] not in DELIMITERS:
            end += 1
        tokens.append(Token("symbol", index, end, source[index:end]))
        index = end
    return tokens


def mark_quoted(tokens: list[Token]) -> list[Token]:
    significant = [i for i, token in enumerate(tokens) if token.kind != "comment"]
    positions = {token_index: order for order, token_index in enumerate(significant)}
    marked = list(tokens)

    def expression(order: int, quote_depth: int) -> int:
        if order >= len(significant):
            return order
        token_index = significant[order]
        token = marked[token_index]
        if token.kind in ("quote", "quasiquote"):
            return expression(order + 1, quote_depth + 1)
        if token.kind == "unquote":
            return expression(order + 1, max(0, quote_depth - 1))
        if token.kind == "open":
            order += 1
            while order < len(significant) and marked[significant[order]].kind != "close":
                order = expression(order, quote_depth)
            return order + 1
        if token.kind in ("symbol", "string"):
            marked[token_index] = dataclasses.replace(token, quoted=quote_depth > 0)
        return order + 1

    order = 0
    while order < len(significant):
        order = expression(order, 0)
    return marked


def comment_alias_spans(token: Token, aliases: dict[str, Alias]) -> list[tuple[int, int, str]]:
    spans: list[tuple[int, int, str]] = []
    index = 0
    text = token.text
    while index < len(text):
        if text[index].isspace() or text[index] in DELIMITERS:
            index += 1
            continue
        end = index + 1
        while end < len(text) and not text[end].isspace() and text[end] not in DELIMITERS:
            end += 1
        atom = text[index:end]
        if atom in aliases:
            spans.append((token.start + index, token.start + end, atom))
        index = end
    return spans


def occurrences(source: str, aliases: dict[str, Alias]) -> list[tuple[int, int, str, str]]:
    found: list[tuple[int, int, str, str]] = []
    for token in mark_quoted(raw_tokens(source)):
        if token.kind == "symbol" and token.text in aliases:
            found.append((token.start, token.end, token.text, "quoted" if token.quoted else "code"))
        elif token.kind == "string":
            value = token.text[1:-1]
            if value in aliases:
                found.append((token.start + 1, token.end - 1, value, "string"))
        elif token.kind == "comment":
            found.extend((*span, "comment") for span in comment_alias_spans(token, aliases))
    return found


def line_and_column(source: str, offset: int) -> tuple[int, int]:
    return source.count("\n", 0, offset) + 1, offset - source.rfind("\n", 0, offset)


def rewrite_source(source: str, aliases: dict[str, Alias]) -> tuple[str, int, list[tuple[int, str, str]]]:
    replacements: list[tuple[int, int, str]] = []
    manual: list[tuple[int, str, str]] = []
    for start, end, old, kind in occurrences(source, aliases):
        if kind in ("code", "comment"):
            replacements.append((start, end, aliases[old].new))
        else:
            line, _ = line_and_column(source, start)
            manual.append((line, kind, old))
    rewritten = source
    for start, end, replacement in reversed(replacements):
        rewritten = rewritten[:start] + replacement + rewritten[end:]
    return rewritten, len(replacements), manual


def parse_categories(value: str) -> set[str]:
    values = set(value.split(","))
    unknown = values.difference(CATEGORIES)
    if unknown:
        raise argparse.ArgumentTypeError(f"unknown categories: {', '.join(sorted(unknown))}")
    return values


def run_scan(args: argparse.Namespace, rewrite: bool) -> int:
    aliases = load_table(args.table)
    excludes = read_excludes(args.exclude, args.exclude_file)
    files = content_files(args.categories, excludes)
    total = 0
    files_with_hits = 0
    manual_count = 0
    for path in files:
        source = path.read_text()
        if rewrite:
            rewritten, count, manual = rewrite_source(source, aliases)
            if count:
                path.write_text(rewritten)
            hits = occurrences(rewritten, aliases)
        else:
            manual = []
            hits = occurrences(source, aliases)
            count = len(hits)
        if count or hits:
            files_with_hits += 1
        total += len(hits) if rewrite else count
        for start, _end, old, kind in hits:
            line, column = line_and_column(rewritten if rewrite else source, start)
            print(f"{path.relative_to(ROOT)}:{line}:{column}: {kind}: {old}")
        for line, kind, old in manual:
            print(f"{path.relative_to(ROOT)}:{line}: manual {kind}: {old}", file=sys.stderr)
            manual_count += 1
    action = "remaining" if rewrite else "found"
    print(f"{action}: {total} old-name occurrences in {files_with_hits} of {len(files)} files")
    if rewrite:
        print(f"manual occurrences not rewritten: {manual_count}")
    return 1 if total or manual_count else 0


def run_parse(args: argparse.Namespace) -> int:
    excludes = read_excludes(args.exclude, args.exclude_file)
    files = content_files(args.categories, excludes)
    command = [
        "cargo", "run", "--quiet", "-p", "eseqlisp", "--bin", "eseqlisp_parse", "--",
        *(path.relative_to(ROOT).as_posix() for path in files),
    ]
    return subprocess.run(command, cwd=ROOT).returncode


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("--table", type=pathlib.Path, default=DEFAULT_TABLE)
    result.add_argument(
        "--categories", type=parse_categories, default=set(CATEGORIES),
        help=f"comma-separated subset of: {', '.join(CATEGORIES)}",
    )
    result.add_argument("--exclude", action="append", default=[], help="repo-relative path prefix")
    result.add_argument("--exclude-file", type=pathlib.Path, help="newline-separated path prefixes")
    subparsers = result.add_subparsers(dest="command", required=True)
    extract = subparsers.add_parser("extract", help="regenerate the durable old-to-new table")
    extract.add_argument("--check", action="store_true", help="fail instead of updating a stale table")
    subparsers.add_parser("check", help="report old spellings at Lisp symbol boundaries")
    subparsers.add_parser("rewrite", help="rewrite code symbols and documentation comments")
    subparsers.add_parser("parse", help="reader-parse every selected content file")
    return result


def main() -> int:
    args = parser().parse_args()
    if args.command == "extract":
        aliases = extract_aliases()
        temporary = args.table.with_suffix(args.table.suffix + ".new")
        write_table(temporary, aliases)
        if args.check:
            same = args.table.exists() and args.table.read_bytes() == temporary.read_bytes()
            temporary.unlink()
            if not same:
                print(f"stale alias table: {args.table.relative_to(ROOT)}", file=sys.stderr)
                return 1
        else:
            temporary.replace(args.table)
        print(f"extracted {len(aliases)} unique aliases with no conflicting targets")
        return 0
    if args.command == "check":
        return run_scan(args, rewrite=False)
    if args.command == "rewrite":
        return run_scan(args, rewrite=True)
    if args.command == "parse":
        return run_parse(args)
    raise AssertionError(args.command)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(2)
