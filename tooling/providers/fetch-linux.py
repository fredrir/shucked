#!/usr/bin/env python3
import hashlib
from pathlib import Path
import urllib.request

DEST = Path(__file__).resolve().parents[2] / 'target/provider-runtime/sources'
SOURCES = [
    ('https://ftp.gnu.org/gnu/bash/bash-5.3.tar.gz', '0d5cd86965f869a26cf64f4b71be7b96f90a3ba8b3d74e27e8e9d9d5550f31ba'),
    ('https://www.zsh.org/pub/old/zsh-5.9.tar.xz', '9b8d1ecedd5b5e81fbf1918e876752a7dd948e05c1a0dba10ab863842d45acd5'),
    ('https://github.com/fish-shell/fish-shell/releases/download/4.9.3/fish-4.9.3.tar.xz', '20998a25f73217ddcc19f499055fd587e9912d1ad6e7109120fbcf2871f0b98c'),
]

if __name__ == '__main__':
    import json
    # Pinned ncurses source metadata is shared with the macOS build lock.
    metadata = json.loads((Path(__file__).with_name('runtime-sources.json')).read_text())
    sources = SOURCES + [(metadata['ncurses']['url'], metadata['ncurses']['sha256'])]
    DEST.mkdir(parents=True, exist_ok=True)
    for url, checksum in sources:
        target = DEST / url.rsplit('/', 1)[1]
        if not target.exists() or hashlib.sha256(target.read_bytes()).hexdigest() != checksum:
            data = urllib.request.urlopen(url, timeout=60).read()
            if hashlib.sha256(data).hexdigest() != checksum:
                raise ValueError(f'checksum mismatch: {url}')
            target.write_bytes(data)
