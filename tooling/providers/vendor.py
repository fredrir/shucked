#!/usr/bin/env python3
"""Refresh pinned completion packs; release packaging never downloads providers."""
import hashlib
import io
import json
from pathlib import Path
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parents[2]
DEST = ROOT / 'tooling/providers/packs'
SOURCES = [
    ('zsh', 'zsh-users/zsh', 'd6bfe888700f47be827e7c6c616284b4c8eadaa0'),
    ('zsh-completions', 'zsh-users/zsh-completions', '67921bc12502c1e7b0f156533fbac2cb51f6943d'),
    ('bash-completion', 'scop/bash-completion', '79d225bad8939a3833314b5af93509131c03f2f8'),
    ('git', 'git/git', '8c9ea59d6eeabbfc5642d99353fce2192645ba1c'),
    ('fish', 'fish-shell/fish-shell', '294f5d9d5652ea241cec19941ecd612bfaea93a1'),
]

EXPECTED = {'zsh': '737f7cc03119fb049870bda6832a66e28baa08a39ee42bc951389dd98e3b4f2e', 'zsh-completions': 'beaaf55bff99735b6ba6f86320dfab81a18a17e56662d7e7d30e6643664e33bf', 'bash-completion': '5c4eb6428d6deca63c83154c5d5a8d44c902d3eec5f9e340a37994d4c5f69253', 'fish': '67a21a989e5da24e644c56ba4e9b938efd98843f900d94f73d64b43ae72d3157'}

EXPECTED['git'] = '231b9535021c47054487ffe9fbf9ac4c43ad5c83e1ea073b1cdefc9ff90972a5'

def selected(name, rel):
    if 'shellcheck' in rel.lower():
        return False
    if rel in ('LICENCE', 'LICENSE', 'COPYING', 'COPYING.bash', 'COPYING.GPL2', 'COPYING.LGPL2.1'):
        return True
    if name == 'git':
        return rel == 'contrib/completion/git-completion.bash'
    if name == 'zsh':
        return rel.startswith('Completion/') and '/.' not in rel and (Path(rel).name.startswith('_') or Path(rel).name in ('compinit', 'compaudit', 'compdump', 'compinstall'))
    if name == 'zsh-completions':
        return rel.startswith('src/_')
    if name == 'bash-completion':
        return rel == 'bash_completion' or rel.startswith('completions/') or rel.startswith('helpers/')
    return rel.startswith('share/completions/') or rel.startswith('share/functions/')


def main():
    manifest = []
    for name, repo, revision in SOURCES:
        url = f'https://codeload.github.com/{repo}/tar.gz/{revision}'
        data = urllib.request.urlopen(url, timeout=60).read()
        if hashlib.sha256(data).hexdigest() != EXPECTED[name]:
            raise ValueError(f'archive checksum mismatch: {name}')
        entry = dict(name=name, repository=f'https://github.com/{repo}', revision=revision,
                     source=url, sha256=hashlib.sha256(data).hexdigest(), files=[])
        with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
            for member in archive.getmembers():
                rel = member.name.partition('/')[2]
                if not (member.isfile() or member.issym() or member.islnk()) or not selected(name, rel):
                    continue
                relative = Path(rel)
                if relative.is_absolute() or '..' in relative.parts:
                    raise ValueError('unsafe archive path')
                pack_path = f'{name}/{rel}'
                if name == 'git' and rel == 'contrib/completion/git-completion.bash':
                    pack_path = 'bash-completion/completions/git'
                target = DEST / pack_path
                target.parent.mkdir(parents=True, exist_ok=True)
                stream = archive.extractfile(member)
                if stream is None:
                    continue
                payload = stream.read()
                target.write_bytes(payload)
                entry['files'].append(dict(path=pack_path, sha256=hashlib.sha256(payload).hexdigest()))
        manifest.append(entry)
    (DEST / 'manifest.json').write_text(json.dumps(dict(schemaVersion=1, sources=manifest), indent=2) + '\n')

if __name__ == '__main__':
    main()
