#!/usr/bin/env python3
"""Fetch pinned Unix runtime inputs; metadata is never refreshed during builds."""
import hashlib
import json
import os
from pathlib import Path
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
DEST = Path(os.environ.get('SHUCKED_PROVIDER_DEST', ROOT / 'target/provider-runtime')) / 'sources'

if __name__ == '__main__':
    metadata = json.loads(Path(__file__).with_name('runtime-sources.json').read_text())
    DEST.mkdir(parents=True, exist_ok=True)
    for name, item in metadata.items():
        for index, spec in enumerate([item, *item.get('patches', [])]):
            url, checksum = spec['url'], spec['sha256']
            filename = url.split('?', 1)[0].rsplit('/', 1)[1]
            target = DEST / filename
            if not target.exists() or hashlib.sha256(target.read_bytes()).hexdigest() != checksum:
                data = urllib.request.urlopen(url, timeout=60).read()
                if hashlib.sha256(data).hexdigest() != checksum:
                    raise ValueError(f'checksum mismatch: {url}')
                target.write_bytes(data)
            print(f'verified {name}: {filename}', flush=True)
