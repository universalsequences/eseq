#!/usr/bin/env python3
"""Verify bundled factory audio, or restore exact upstream bytes with --fetch."""

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import tempfile
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--fetch', action='store_true', help='restore missing payloads')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1] / 'content/packages/universalsequences.factory-samples'
    provenance = json.loads((root / 'provenance.json').read_text())
    files = {}
    for entry in provenance['files']:
        relative = PurePosixPath(entry['path'])
        if relative.is_absolute() or '..' in relative.parts or entry['path'] in files:
            raise ValueError(f"Invalid or duplicate path: {relative}")
        files[entry['path']] = entry
        path = root / relative
        if not path.exists() and args.fetch:
            with urllib.request.urlopen(entry['url'], timeout=60) as response:
                data = response.read(entry['bytes'] + 1)
            if len(data) != entry['bytes'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
                raise ValueError(f"Upstream content mismatch: {relative}")
            path.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as output:
                temporary = Path(output.name)
                output.write(data)
            try:
                temporary.replace(path)
            finally:
                temporary.unlink(missing_ok=True)
        data = path.read_bytes()
        if len(data) != entry['bytes'] or hashlib.sha256(data).hexdigest() != entry['sha256']:
            raise ValueError(f"Bundled content mismatch: {relative}")

    manifest = [json.loads(line) for line in (root / 'samples.jsonl').read_text().splitlines()]
    samples = [entry for entry in manifest if 'hash' in entry]
    expected = {path for path, entry in files.items() if 'title' in entry}
    actual = {'samples/' + sample['path'] for sample in samples}
    if actual != expected or len(actual) != len(samples):
        raise ValueError('Sample manifest differs from the audited selection')
    for sample in samples:
        entry = files['samples/' + sample['path']]
        if (sample['hash'] != entry['sha256'] or sample['title'] != entry['title']
                or sample['tags'] != entry['tags']
                or sample['source'] != 'pkg:universalsequences.factory-samples/' + entry['source']):
            raise ValueError(f"Sample metadata mismatch: {sample['path']}")
    actual_payloads = {p.relative_to(root).as_posix() for p in (root / 'samples').rglob('*') if p.is_file()}
    if actual_payloads != expected:
        raise ValueError('Unaudited or missing audio in the factory sample package')
    library = (';; Factory sample identities for scripts; importing does not load audio.\n'
               '(module universalsequences.factory-samples.library)\n(export samples)\n\n'
               '(def samples (dict\n')
    library += ''.join('  ' + json.dumps(sample['title']) + ' '
                       + json.dumps('samples/' + sample['hash'] + '.wav') + '\n'
                       for sample in samples)
    library += '))\n'
    if (root / 'src/library.lisp').read_text() != library:
        raise ValueError('Lisp sample references differ from the audited manifest')
    total = sum(entry['bytes'] for entry in files.values() if 'title' in entry)
    print(f'Verified {len(samples)} factory samples ({total:,} bytes) and bundled source notices.')


if __name__ == '__main__':
    main()
