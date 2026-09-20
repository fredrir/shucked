#!/usr/bin/env python3
"""Refresh pinned completion packs; release packaging never downloads providers."""
import hashlib
import argparse
import io
import json
import re
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
    ('oh-my-zsh', 'ohmyzsh/ohmyzsh', '6421f8e104e4e87f2373362cbf61e46a918612a7'),
    ('brew', 'Homebrew/brew', '13eeb3b76e7fde4cc6f7084487946ea8ea940ada'),
    ('eza', 'eza-community/eza', '471bfbc7b03cbac8c738e8d9050edb06ee79132a'),
    ('paru', 'Morganamilo/paru', '9ac3578807a87858651e81a02586ceb947686e7c'),
]

EXPECTED = {'zsh': '737f7cc03119fb049870bda6832a66e28baa08a39ee42bc951389dd98e3b4f2e', 'zsh-completions': 'beaaf55bff99735b6ba6f86320dfab81a18a17e56662d7e7d30e6643664e33bf', 'bash-completion': '5c4eb6428d6deca63c83154c5d5a8d44c902d3eec5f9e340a37994d4c5f69253', 'fish': '67a21a989e5da24e644c56ba4e9b938efd98843f900d94f73d64b43ae72d3157'}

EXPECTED['git'] = '231b9535021c47054487ffe9fbf9ac4c43ad5c83e1ea073b1cdefc9ff90972a5'
EXPECTED.update({
    'oh-my-zsh': 'd11e4cfd5f33a1cfdf5fa09e77d38f5633341c989fdd5349f46965a07ab92071',
    'brew': 'bfe6191ca073a4058fa28861ab3f721a891e94ddf76a7e4be57dddb793b47661',
    'eza': 'e79ca92ade932673c250a43f494da8bb67bec64fab90e2e0d533b5c8f51e84ff',
    'paru': 'fa1e2da402283f9584f98458c61ae8d0bb04c92b2cddb1d2727c8ce944da457e',
})

def selected(name, rel):
    if 'shellcheck' in rel.lower():
        return False
    if rel in ('LICENCE', 'LICENSE', 'LICENSE.txt', 'LICENSE-MIT', 'COPYING', 'COPYING.bash', 'COPYING.GPL2', 'COPYING.LGPL2.1'):
        return True
    if name == 'oh-my-zsh':
        return rel.startswith('plugins/') and (Path(rel).name.startswith('_') or Path(rel).name in ('LICENSE', 'LICENSE.txt'))
    if name in ('brew', 'eza'):
        return rel.startswith('completions/zsh/_')
    if name == 'paru':
        return rel == 'completions/zsh'
    if name == 'git':
        return rel == 'contrib/completion/git-completion.bash'
    if name == 'zsh':
        return rel.startswith('Completion/') and '/.' not in rel and (Path(rel).name.startswith('_') or Path(rel).name in ('compinit', 'compaudit', 'compdump', 'compinstall'))
    if name == 'zsh-completions':
        return rel.startswith('src/_')
    if name == 'bash-completion':
        return rel == 'bash_completion' or rel.startswith('completions/') or rel.startswith('helpers/')
    return rel.startswith('share/completions/') or rel.startswith('share/functions/')


def destination(name, rel):
    if name == 'git' and rel == 'contrib/completion/git-completion.bash':
        return 'bash-completion/completions/git'
    if name in ('oh-my-zsh', 'brew', 'eza', 'paru'):
        if rel == 'completions/zsh':
            return 'zsh-extra/_paru'
        if Path(rel).name.startswith('_'):
            return f'zsh-extra/{Path(rel).name}'
    return f'{name}/{rel}'


def registry(manifest):
    commands = {}
    files = [(source['name'], item['path']) for source in manifest for item in source['files']]
    files.sort(key=lambda item: (not item[1].startswith('zsh-extra/'), item[1]))
    for source, relative in files:
        path = DEST / relative
        if 'shellcheck' in relative.lower():
            continue
        if path.name.startswith('_') and relative.startswith(('zsh/', 'zsh-completions/', 'zsh-extra/')):
            first = path.read_text(errors='replace').splitlines()[:1]
            if not first or not first[0].startswith('#compdef '):
                continue
            names = first[0][len('#compdef '):].split('#')[0].split()
            if not names:
                continue
            if names[0].startswith('-'):
                continue
            names = [name.split('=')[0] for name in names if not name.startswith('-')]
            engine = 'zsh'
        elif relative.startswith('bash-completion/completions/') and path.name not in ('Makefile.am', 'Makefile.in'):
            names, engine = [path.name.lstrip('_')], 'bash'
        elif relative.startswith('fish/share/completions/') and path.suffix == '.fish':
            if not any(line.strip() and not line.lstrip().startswith('#') for line in path.read_text().splitlines()):
                continue
            names, engine = [path.stem], 'fish'
        else:
            continue
        entry = dict(engine=engine, path=relative, source=source)
        for name in names:
            if re.fullmatch(r'[\w.+:/@-]+', name):
                commands.setdefault(name, []).append(entry)
    priority = {'zsh': 0, 'bash': 1, 'fish': 2}
    for entries in commands.values():
        entries.sort(key=lambda entry: priority[entry['engine']])
    return dict(schemaVersion=1, zshPaths=['zsh-extra'], commands=dict(sorted(commands.items())))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', action='append', choices=[source[0] for source in SOURCES])
    parser.add_argument('--registry-only', action='store_true')
    args = parser.parse_args()
    previous = json.loads((DEST / 'manifest.json').read_text())['sources'] if (DEST / 'manifest.json').exists() else []
    manifest = []
    for name, repo, revision in SOURCES:
        if args.registry_only or (args.source and name not in args.source):
            manifest.extend(source for source in previous if source['name'] == name)
            continue
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
                pack_path = destination(name, rel)
                target = DEST / pack_path
                target.parent.mkdir(parents=True, exist_ok=True)
                stream = archive.extractfile(member)
                if stream is None:
                    continue
                payload = stream.read()
                target.write_bytes(payload)
                entry['files'].append(dict(path=pack_path, sha256=hashlib.sha256(payload).hexdigest()))
        new_paths = {item['path'] for item in entry['files']}
        for old in previous:
            if old['name'] == name:
                for item in old['files']:
                    if item['path'] not in new_paths:
                        (DEST / item['path']).unlink(missing_ok=True)
        manifest.append(entry)
    (DEST / 'registry.json').write_text(json.dumps(registry(manifest), indent=2) + '\n')
    generated = [dict(path='registry.json', sha256=hashlib.sha256((DEST / 'registry.json').read_bytes()).hexdigest())]
    (DEST / 'manifest.json').write_text(json.dumps(dict(schemaVersion=1, sources=manifest, generated=generated), indent=2) + '\n')

if __name__ == '__main__':
    main()
