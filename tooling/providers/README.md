# Managed shell providers

| Component | Pinned source | Purpose |
|---|---|---|
| Zsh | 5.9 | Completion engine and core definitions |
| zsh-completions | 0.35.0 | Additional Zsh definitions |
| bash-completion | 2.16.0 | Bash definitions and helpers |
| Git | 2.49.0 | Git's Bash completion definition |
| Fish definitions | 4.0.2 | Fish definitions and helpers |
| Bash runtime | macOS 5.3.20; Linux recipe 5.3 | Managed Bash engine |
| Fish runtime | 4.9.3 | Managed Fish engine |

| Contract | Behavior |
|---|---|
| Delivery | `bin/providers/packs` and `bin/providers/runtime` in the VSIX |
| Integrity | SHA-256 for source archives and every packaged file |
| Startup | Managed engines bypass personal startup and completion files |
| History | Fish private mode; no history completion in these workers |
| Editor input | Positional arguments or quoted buffer data; never evaluated as a command |
| Native callbacks | Trusted completion code can run helpers; workspace trust and deadlines required |
| Discovery | Private shell engines do not extend the target command search PATH |
| Runtime override | `SHUCKED_PROVIDER_ROOT`, absolute path with a pack manifest |
| Licenses | Original license files remain with packs; runtime sources, patches, build metadata, and Rust dependencies accompany binaries |
| Pack refresh | Explicit maintenance operation; packaging verifies local assets without downloading |
| Unsupported runtime | Packaging fails instead of omitting a required engine |

| Platform | Current evidence |
|---|---|
| macOS arm64 | Relocated Bash/Fish engines pass worker smoke tests; source-built Zsh completion needs further runtime validation |
| Linux GNU/musl | Target-host source build recipe; requires execution on the actual target before release |
| macOS x64 | Requires a matching runtime lock and target-host build |
| Windows | Bounded Job Object process capture implemented; compatible shell runtime distribution still required |
| Minimal Unix hosts | Standard POSIX helper utilities remain required; a fully private helper closure is not yet supplied |

```sh
# Refresh definitions from immutable revisions and verified archive hashes.
python3 tooling/providers/vendor.py

# macOS arm64; installed Homebrew versions must match the runtime lock.
tooling/providers/build-zsh.sh
python3 tooling/providers/bundle-runtime.py

# Linux target host; C/Rust toolchains, CMake, make, curl, Python, and POSIX tools required.
tooling/providers/build-linux.sh

# Exercise the actual private engines with hostile personal startup files.
python3 -m unittest discover -s tooling/providers/tests -v
```
