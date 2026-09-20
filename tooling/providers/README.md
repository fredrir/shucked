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
| GNU helpers | `runtime-sources.json` | Private coreutils, findutils, grep, sed, awk |

| Contract | Behavior |
|---|---|
| Delivery | `bin/providers/packs` and `bin/providers/runtime` in the VSIX |
| Integrity | Schema 2; exact target, SHA-256 file inventory, corresponding sources, SPDX SBOM, passed worker receipt |
| Startup | Managed engines bypass personal startup and completion files |
| History | Fish private mode; no history completion in these workers |
| Editor input | Positional arguments or quoted buffer data; never evaluated as a command |
| Native callbacks | Trusted completion code can run helpers; workspace trust and deadlines required |
| Discovery | Private engines/helpers extend only worker PATH; target PATH stays separate; helper-only command candidates are removed |
| Runtime override | `SHUCKED_PROVIDER_ROOT`, absolute path with a pack manifest |
| Licenses | Original license files remain with packs; runtime sources, patches, build metadata, and Rust dependencies accompany binaries |
| SBOM license IDs | Pinned SPDX3.27 identifiers; ambiguous upstream labels stay in manifest and become `NOASSERTION` in SPDX |
| Pack refresh | Explicit maintenance operation; packaging verifies local assets without downloading |
| Unsupported runtime | Packaging fails instead of omitting a required engine |

| Platform | Current evidence | Release gate |
|---|---|---|
| macOS arm64 | Relocated private Zsh/Bash/Fish and 115 helper names pass hostile-startup tests with restricted PATH | Passed locally |
| Linux GNU arm64 | Prior source-built engines/helpers and relocated worker tests passed in Podman | Revalidate latest worker revision |
| Linux musl arm64 | Prior source-built engines/helpers and relocated worker tests passed in Alpine Podman | Revalidate latest worker revision |
| Linux GNU/musl x64 | Source-built engines/helpers in native Docker on Arch x64; relocated workers and strict inventories pass | Passed on native x64 host |
| Linux GNU armhf | Digest-pinned ARMv7 container recipe; target-native source build supports ARMHF | Full build and target execution required |
| macOS x64 | Native target-host source recipe; no matching local execution host | Target execution required |
| Windows x64 | Pinned MSYS engine/helper packages; 363 x64 PE images checked for DLL availability; full binary/source closure staged | Windows worker/containment validation required |
| Windows ARM64 | Same x64 MSYS package closure; Windows 11 x64 emulation required | ARM64-host worker/containment validation required |

Windows runtime dispatch, path conversion, and process containment are implemented. Native worker, DLL search, and containment execution remain unverified. Both Windows targets remain blocked from release.

| Build input | Value |
|---|---|
| Unix source pins | `runtime-sources.json` |
| Windows binary/source pins | `runtime-lock.msys-x64.json` |
| Output | `SHUCKED_PROVIDER_DEST`; default `target/provider-runtime` |
| Build directory | `SHUCKED_PROVIDER_BUILD`; target-specific directory required |
| Parallel jobs | `SHUCKED_BUILD_JOBS`; use 1 in small VMs |
| Execution receipt | `SHUCKED_PROVIDER_EXECUTION`: native, container, or emulated |
| Optional Fish cross build | `SHUCKED_FISH_PREBUILT`: prefix visible to builder with `source.json`, binary/share files, vendored Rust sources; source, binary, vendor SHA and target checked |
| Root-only containers | Explicit `FORCE_UNSAFE_CONFIGURE=1` for GNU configure; ordinary builds use an unprivileged account |
| Source changes | Archive hashes and patches verified before build |
| Build reproducibility | Pinned inputs and fixed source epoch; byte-identical output across toolchains is not asserted |
| Release rejection | Unbuilt/untested targets, missing sources/helpers/SBOM, mismatched targets, changed inventory, escaping links |

```sh
# Refresh definitions from immutable revisions and verified archive hashes.
python3 tooling/providers/vendor.py

# macOS arm64; installed Homebrew versions must match the runtime lock.
tooling/providers/build-zsh.sh
python3 tooling/providers/bundle-runtime.py

# Unix target host; C/Rust toolchains, CMake, make, curl, Python, and POSIX tools required.
tooling/providers/build-unix.sh

# Repeat the validated ARM64 container builds from pinned base images; x64 variants use the same commands with an x64 suffix.
tooling/providers/build-container.sh linux-arm64
tooling/providers/build-container.sh alpine-arm64

# Stage Windows x64 packages; this does not produce a releasable artifact.
python3.14 tooling/providers/bundle-msys.py --target win32-x64 --output target/provider-windows-x64

# Explicit source/binary lock refresh, never part of packaging.
python3 tooling/providers/refresh-msys-lock.py

# Exercise the actual private engines with hostile personal startup files.
python3 -m unittest discover -s tooling/providers/tests -v

# Repeat against an isolated installed VSIX, including relocated private libraries.
SHUCKED_TEST_PROVIDER_ROOT=/path/to/installed/extension/bin/providers \
  python3 -m unittest discover -s tooling/providers/tests -v
```
