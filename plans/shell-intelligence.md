# Environment-aware shell intelligence implementation plan

Status: implementation in progress. The original acceptance gates below remain binding; implementation availability does not mean platform release validation is complete.

## Implementation evidence

| Area                   | Implemented                                                                                                                                                          | Remaining gate                                                                                    |
| ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| Shared resolution      | `shucked-command`, source-backed semantic facts, exact PATH evidence, tri-state results; independent review and regression tests                                     | WAN and cold filesystem-cache measurements                                                        |
| Editor feedback        | Command tokens, resolution hover, parser/environment workers, snapshot-checked corrections; installed release VSIX18 checks pass on macOS arm64 and native Linux x64 | Other platform/editor acceptance                                                                  |
| Context                | Workspace/Portable, captured targets, terminal selection, explicit cwd, untitled association                                                                         | Real WSL and remote VS Code UI; SSH/container protocol checks pass                                |
| Validators             | Brew/Git inventories; exact-version eza/rg/fd/bat/GNU/BSD ls/Pacman/curl/SSH/Docker/kubectl grammar                                                                  | Uncovered versions/extensions remain Unknown; finite coverage recorded in validator manifests     |
| Providers              | Vendored definitions; managed Zsh/Bash/Fish adapters; private engines/helpers validated on macOS arm64 and native Linux GNU/musl x64                                 | Intel macOS, ARMHF and Windows execution; ARM64 final receipts explicitly emulated                |
| Fish                   | Dedicated command/syntax frontend and completion routing                                                                                                             | Broader Fish grammar fixtures; full existing lint-rule parity is not claimed                      |
| Refresh/dependencies   | Host filesystem watches, relative PATH/cwd contexts, coalesced refresh, scoped declarations and guards                                                               | Unsupported filesystems use bounded polling fallback                                              |
| Live sessions          | Authenticated Bash/Zsh/Fish prompt hooks and transient custom completers; real shell/editor fixtures                                                                 | Unix signal channel only; Windows custom-completer channel unavailable                            |
| Inventories/comparison | Versioned bounded capture/import, audited capability evidence, target-labelled argument comparison                                                                   | Uncovered capabilities remain Unknown                                                             |
| History                | Separate opt-ins, actual custom history paths, bounded memory, authenticated accepted-command confirmation                                                           | Real editor insertion/revocation pass on macOS arm64 and Linux x64                                |
| Distribution           | Pinned hashes, licenses/source archives, exact runtime architecture checks; Windows dispatch cross-checks; native Linux GNU/musl x64 runtime artifacts               | Release VSIX18/18 on macOS arm64/Linux x64; Intel macOS, ARMHF and Windows execution remain gated |

Evidence is recorded by tests in the corresponding crates, `tests/lsp/test_command_intelligence.py`, `editors/vscode/tests`, and `tooling/providers/tests`. Platform availability and build commands are tracked in [provider documentation](../tooling/providers/README.md).

## Local performance evidence

| Measurement                          | Fixture                                                                         | Debug p50 / p95    |
| ------------------------------------ | ------------------------------------------------------------------------------- | ------------------ |
| Warm command completion              | 4 PATH directories, 1,001 executables, 100 functions; 1,000 returned candidates | 14.797 / 15.241 ms |
| Cached native argument completion    | 128 described flags                                                             | 3.572 / 3.757 ms   |
| Full-text edit → completion response | 103 lines, 2,940 bytes; 1,000 returned candidates                               | 24.325 / 25.227 ms |

macOS arm64, local stdio, 10 warmups and 100 samples. These figures include the Python client, exclude cold provider startup and remote transport, and do not measure notification handling alone. Run `python3 tests/lsp/benchmark_command_intelligence.py --build-mode debug`; output records binary SHA, platform, and fixture sizes. Timing thresholds are not unit-test assertions.

## Remote performance evidence

| Measurement                      | SSH p50 / p95        | Container p50 / p95  |
| -------------------------------- | -------------------- | -------------------- |
| Startup through first completion | 125.842 / 128.870 ms | 119.044 / 123.239 ms |
| Cold command request             | 30.928 / 31.767 ms   | 35.048 / 38.123 ms   |
| First native help request        | 11.965 / 12.417 ms   | 11.361 / 11.570 ms   |
| Warm command completion          | 17.567 / 18.615 ms   | 19.050 / 20.180 ms   |
| Warm native arguments            | 3.914 / 4.019 ms     | 4.858 / 5.286 ms     |
| Edit through completion          | 28.243 / 29.629 ms   | 26.441 / 29.433 ms   |

Linux ARM64 debug server in a local VM; 10 fresh processes, 10 warmups and 100 warm samples. Four PATH directories, 1,001 executable fixtures, 100 functions and 128 native-help candidates. Server caches start fresh for cold runs; filesystem caches remain warm. No simulated WAN latency. Reproduce with `tests/remote/benchmark_completion.py`; JSON output records binary identity and fixtures. Real SSH/container acceptance also checks installation refresh and reconnect with absolute and relative PATH.

## Arch x86_64 SSH evidence

| Check                            | Result                                                                                                                                                           |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Real host                        | Arch Linux x86_64 over SSH from macOS arm64                                                                                                                      |
| Absolute and relative PATH       | Host inventory, paths, install refresh and reconnect passed; 1.038 s each                                                                                        |
| Native package/flag completion   | Real pacman repository packages and bundled Git flags through Bash/Zsh/Fish LSP; exact insertion; hostile personal startup files ignored; debug and release pass |
| Startup through first completion | Debug p50 56.381 ms; p95 57.649 ms                                                                                                                               |
| Warm command completion          | Debug p50 14.611 ms; p95 18.432 ms                                                                                                                               |
| Warm native arguments            | Debug p50 3.586 ms; p95 3.977 ms                                                                                                                                 |
| Edit through completion          | Debug p50 21.881 ms; p95 23.057 ms                                                                                                                               |
| Measurement scope                | Same fixture and sample counts as remote benchmark above; actual SSH, warm filesystem caches                                                                     |
| Unavailable hosts                | User confirmed no Windows/WSL or Intel macOS host                                                                                                                |

## Final integrated validation

| Layer                             | Result                                                                                                                                                     |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Shared command resolver/processes | 47 tests passed                                                                                                                                            |
| LSP Rust                          | 252 unit and 33 integration tests passed; one manual latency probe ignored                                                                                 |
| LSP protocol                      | 52 tests passed against freshly rebuilt binaries                                                                                                           |
| Extension                         | 37 tests passed, including real Bash/Zsh/Fish callbacks                                                                                                    |
| Static checks                     | Formatting and three-crate all-target Clippy passed                                                                                                        |
| Windows cross-check               | x64 GNU target tests compile and Clippy passed; no Windows execution claim                                                                                 |
| Installed macOS arm64             | Release VSIX18/18, VS Code1.138.0; isolated secret storage/profile                                                                                         |
| Installed Linux x64               | Release and debug VSIX18/18 on native Arch hardware, VS Code1.138.0 in isolated Docker; release built against Debian/glibc2.36                             |
| Runtime artifacts                 | macOS arm64 and GNU/musl Linux x64 relocation and strict inventory checks passed natively; latest GNU/musl ARM64 receipts passed under isolated QEMU/proot |
| Provider tooling                  | 8 JavaScript and 11 Python tests passed; GNU managed-runtime minimum glibc2.36                                                                             |
| Review                            | Independent review completed; insertion, ownership, PE architecture and source-hint test findings fixed                                                    |

Local artifact receipts under `target/acceptance/` record binary/VSIX hashes and actual host reports. These files are build outputs, not committed fixtures. Windows live custom-completer transport remains unavailable; Windows/WSL, Intel macOS and ARMHF execution gates remain open.

## Confirmed scope

| Decision                | Value                                                                                                                                                                              |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Default validation      | Workspace-host checks enabled                                                                                                                                                      |
| Portable validation     | Explicit opt-in; syntax and document-semantic checks remain enabled                                                                                                                |
| Delivery                | Completion engines, definitions, required helpers, and defaults ship with the extension; no personal shell configuration required                                                  |
| Workspace execution     | Local, Remote SSH, WSL, and containers use their own extension/workspace host                                                                                                      |
| Personal shell behavior | Applicable interactive context includes aliases/functions/options; standalone scripts retain their interpreter's execution semantics                                               |
| Coverage                | Every Must, Should, and Could below is an implementation milestone, not a deferred wish list                                                                                       |
| Invalidity evidence     | Missing suggestions, provider failure, or incomplete inventory never establishes invalidity                                                                                        |
| Edited documents        | Never execute editor-buffer contents or evaluate a document-directed source graph for analysis; trusted personal startup and live-session capture follow the explicit policy below |
| Fixes                   | Typo corrections require explicit selection; never part of automatic fix-all/on-save                                                                                               |
| Clean room              | No ShellCheck source, documentation, diagnostic wording, or copied output; use approved primary shell/tool documentation and repository-authored fixtures                          |

## Requirement ledger

| ID  | Priority | Deliverable                                                                        | Work packages   |
| --- | -------- | ---------------------------------------------------------------------------------- | --------------- |
| M1  | Must     | One resolver shared by completion, diagnostics, tokens, hover, and fixes           | P1, P2, P5      |
| M2  | Must     | Visible execution context; Workspace default and Portable opt-in                   | P1, P5          |
| M3  | Must     | Shell-correct aliases, source order, shadowing, argument injection, and provenance | P2, P3, P4, P8  |
| M4  | Must     | Resolved / Missing / Unknown with evidence and freshness                           | P1, P2, P6      |
| M5  | Must     | Reliable live parser/environment diagnostics and invalid-command presentation      | P4, P5          |
| M6  | Must     | Authoritative subcommand/flag validation where coverage is sufficient              | P6              |
| M7  | Must     | Explicit, correctly ranged typo fixes for commands, subcommands, and flags         | P5, P6          |
| M8  | Must     | Bundled provider architecture and working Zsh/Bash/Fish adapters                   | P0, P3, P4, P11 |
| S1  | Should   | Broader installed-tool/version/extension coverage                                  | P6, P11         |
| S2  | Should   | Automatic environment refresh after installation/removal/configuration changes     | P7, P8          |
| S3  | Should   | Project declarations for generated, optional, and CI/deployment commands           | P7              |
| S4  | Should   | Flow-sensitive recognition of guarded optional dependencies                        | P2, P7          |
| S5  | Should   | Resolution hovers with target, executable, alias/source, and uncertainty           | P5, P7          |
| C1  | Could    | Live terminal aliases/functions/options/PATH/cwd integration                       | P8              |
| C2  | Could    | Captured target inventories usable offline                                         | P9              |
| C3  | Could    | Comparisons across deployment targets                                              | P9              |
| C4  | Could    | Optional history-based inline suggestions                                          | P10             |

## Baseline gaps addressed by the work packages

| Area                                | Finding                                                                                                              | Required change                                                                     |
| ----------------------------------- | -------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `completion/environment.rs`         | Adds discovery directories beyond inherited PATH; scan failures and partial listings are unsuitable absence evidence | Separate execution search scope from discovery; retain completeness/failure status  |
| `completion/mod.rs`                 | Uses document directory as completion cwd                                                                            | Give runtime cwd an explicit value and known/assumed provenance                     |
| `completion/context.rs`             | Flattens words before native completion                                                                              | Preserve quoting, alias eligibility, expansion structure, and original source spans |
| `completion/native.rs`              | Personal Zsh chiefly handles flags; Brew/Pacman have separate queries                                                | General provider contracts covering commands, arguments, subcommands, and values    |
| `completion/native_process.rs`      | Native execution is Unix-only                                                                                        | Bounded cross-platform worker/process-tree service                                  |
| `handlers/semantic_tokens.rs`       | Primarily classifies syntax and known functions                                                                      | Consume common command analysis; add command/alias/invalid classifications          |
| `handlers/lint.rs`, `did_change.rs` | No environment diagnostics; push generation is synchronous                                                           | Background, versioned analysis and merged push/pull diagnostics                     |
| `handlers/fix.rs`                   | Existing fixes are routed through linter rule identities                                                             | Separate manual environment actions from linter fix-all                             |
| `session/settings.rs`               | Generic LSP syntax errors default off; VS Code defaults on                                                           | Default syntax diagnostics on consistently; retain explicit opt-out                 |
| Parser profiles                     | POSIX/Mksh/Bash/Zsh exist; Fish can fall back to Bash                                                                | Explicit Fish frontend and language routing; prohibit silent fallback               |
| Distribution                        | Cargo release targets and extension platform allowlist differ                                                        | One tested support matrix for server, runtime packs, and installed VSIX             |

## Shared contracts and ownership

Proposed ownership keeps host execution out of semantic analysis and allows CLI target capture without depending on editor internals.

| Owner                           | Contract / responsibility                                                                                                                                 |
| ------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `shucked-semantic`              | Command-site/source/scope/alias/PATH/cwd/guard facts; no native subprocess calls                                                                          |
| New `shucked-command` crate     | Execution-context schemas, snapshots, resolver, provider protocols, validators, target capture/comparison; pure analysis and host I/O in separate modules |
| `shucked-lsp`                   | Snapshot scheduling, cancellation, client capabilities, diagnostic/token/hover/edit translation                                                           |
| `editors/vscode`                | Context selector, semantic styling, terminal attachment, comparison UI, inline suggestions                                                                |
| `shucked-cli`                   | Explicit capture/import/compare entry points; ordinary lint does not silently acquire host-dependent results                                              |
| `tooling` and release packaging | Pinned provider builds, manifests, dependency/license artifacts, platform tests and SBOM                                                                  |

| Schema                   | Required fields                                                                                                                                                                                            |
| ------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ExecutionContext`       | Target ID, workspace association, document dialect, target interpreter identity/version/options, script/startup-file/interactive-session mode, validation policy, cwd and its confidence, trust capability |
| `EnvironmentSnapshot`    | Exact runtime PATH/search semantics, builtin/alias/function/executable inventory, declarations, provenance, completeness, failures, generation and timestamp                                               |
| `CommandSiteFacts`       | Static/dynamic name, lexical form, source ranges, applicable definitions/aliases, effective arguments, environment effects, guard and wrapper context                                                      |
| `CommandResolution`      | Resolved identity/kind/provenance/effective words; Missing with authoritative search evidence; Unknown with reason/possible identities                                                                     |
| `ProviderRequest/Result` | Canonical invocation, cursor mapping, host/context identity, pack/runtime identity, candidates/edits/descriptions, completeness and failures                                                               |
| `ValidationEvidence`     | Actual executable identity/version/vendor/platform, extension inventory, exact grammar/context coverage, provenance and freshness; Valid / Invalid / Unknown                                               |
| `ResolutionSnapshotKey`  | Document URI/version, analysis generation, target/environment generation, provider-pack generation                                                                                                         |
| `TargetInventory`        | Versioned portable schema, target label/platform/interpreter, capture age, inventory/capability completeness and privacy metadata                                                                          |

### Invariants

- An exact point lookup may establish absence only when the applicable search scope is known and inspected successfully. Truncated/unreadable/stale/failed inventories yield Unknown.
- Private runtime/helper binaries never become evidence that a command exists in the target environment. Preserve empty and relative PATH entries according to the selected cwd.
- A compatible completion engine is an implementation detail; it must not override the document interpreter's language/execution semantics.
- Generated alias words carry mappings back to original tokens. Unsupported complex aliases and ambiguous functions remain Unknown for downstream tool-grammar validation.
- Capture inventories without executing candidate programs. Probe versions/metadata only through audited read-only provider interfaces, never arbitrary `tool --version` or edited arguments.
- Completion metadata is suggestions-only unless a distinct validator supplies complete applicable evidence. Plugin-extensible tools require extension discovery before rejection.
- Project declarations record expected availability, not a claim that the executable is installed. Diagnose an unmet declared dependency separately from a spelling mistake.
- Every editor result uses a matching immutable snapshot. Older document/target/provider results cannot overwrite newer ones.
- Native workers obey the existing trust boundary. Workspace settings cannot elevate initialization execution permission.
- Managed providers ignore personal startup files. Selecting My shell or starting a Shucked terminal may initialize trusted on-disk personal startup code; this is real code execution, never a per-edit analysis technique. Attached-session capture reads state without replaying the commands that created it.
- Startup-file editing uses source-ordered static analysis over an explicit entry context. Do not automatically reload the open startup file or its source graph, and do not treat post-startup aliases/functions as proof that they existed before their definitions in that file. Unsupported entry state remains Unknown.

## Work packages

| Package                                      | Deliverables                                                                                                                                                                                           | Dependencies                                             | Acceptance gate                                                                                                                                                                     |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| P0 — Contracts and feasibility               | Freeze schemas, context precedence, platform matrix, pack layout, benchmark harness and decision records; prototype Windows runtime delivery and shell-hook coexistence                                | None                                                     | All teams can work against fixtures; runtime/platform limitations and decision gates are explicit                                                                                   |
| P1 — Context and environment service         | Add shared crate; exact runtime inventory versus discovery; immutable generations; direct lookups; Workspace/Portable/My shell/named contexts; configuration precedence and persistence                | P0                                                       | Local/remote/multi-root isolation, true PATH/cwd semantics, Unknown on incomplete evidence, Portable changes policy without changing parser dialect                                 |
| P2 — Bash/Zsh/POSIX command semantics        | Reuse command topology, function resolution, source effects, CFG and value facts; add aliases/unalias/options, argument injection, source-order visibility, PATH/cwd effects, wrappers and guard facts | P0, P1 interfaces                                        | Shared resolver handles functions, sources, aliases, builtins, executables, quoting and supported flow; uncertain execution contexts never get guessed                              |
| P3 — Managed providers and bundles           | General process service; Zsh/Bash/Fish adapters; pinned runtime/definition/helper packs; private namespaces; cache/provenance; authoritative trust and cancellation                                    | P0, P1 interfaces                                        | Empty HOME, no installed completion framework or shell runtime, offline first run; command/flag/subcommand/value/path candidates; no leaked worker trees                            |
| P4 — Fish document frontend                  | Explicit dialect/language registration; Fish syntax/recovery, command and expansion facts, functions/scopes, sources, wrappers, cwd/PATH effects and supported guards; feed shared command contracts   | P0, P1 interfaces; integrate with P2/P3                  | Fish-specific syntax and command intelligence pass independently; no Bash parser fallback or accidental Bash formatting of Fish                                                     |
| P5 — Editor intelligence                     | Asynchronous diagnostics, syntax defaults, semantic classifications/styles, provenance hover, manual typo actions, context picker and target status                                                    | P1, P2; P4 for Fish; P6 validators via interface         | User examples work; push/pull agree; slow providers do not block edits; stale results/fixes cannot apply; theme-independent warning diagnostics remain available                    |
| P6 — Tool identity and validators            | Structured capability APIs plus version-scoped manifests; exact executable/vendor/version matching; plugin discovery; reusable validation grammar and compatibility corpus                             | P1, P2, P3; P4 facts for Fish                            | Known grammar typos warn with explicit fixes; unsupported versions/extensions remain Unknown; completion absence never triggers warnings                                            |
| P7 — Refresh, declarations and guards        | Scoped project command declarations; dependency diagnostics; guard analysis integration; executable/package/config watchers, TTL fallback, recapture and refresh action                                | P1, P2, P5, P6 interfaces                                | Install/remove/upgrade updates open documents; declared CI tools and optional guarded commands do not generate false spelling warnings; changes remain scoped                       |
| P8 — Live shell sessions                     | Shipped Bash/Zsh/Fish hooks, optional Shucked terminal profile, explicit attach flow, private authenticated IPC, session generations and metadata snapshots                                            | P0 hook prototype; P1, P2, P3, P5; P4 for Fish documents | Live aliases/functions/options/PATH/cwd update linked documents without rerunning user command text or editing startup files; exit/reconnect marks stale state                      |
| P9 — Target capture and comparison           | Versioned export/import CLI and editor commands; immutable offline inventories; target requirements; multi-target capability matrix, source links and machine-readable report                          | P1, P2, P6; P5 for UI; P7 declarations                   | Offline target analysis never probes local tools for remote evidence; comparisons distinguish Missing/Unknown/version differences; partial/corrupt/stale imports handled explicitly |
| P10 — Optional history suggestions           | Separate opt-ins for session history and existing history files; shell-specific readers, bounded local index, context/prefix ranking, native-result merging, inline UI and provenance                  | P1, P3, P5; P8 session capture; P4 Fish syntax           | Off means no collection/read/storage; enabled suggestions remain host/context scoped; acceptance only inserts text; no stale or incorrectly quoted edits                            |
| P11 — Release, parity and independent review | Reconciled builds, redistributable/source/license artifacts, installed-VSIX tests, real remote tests, performance/resource regression gates, user documentation and migration                          | P1–P10                                                   | Every requirement ledger item has passing evidence; every advertised platform has an explicit tested support result                                                                 |

### Provider and validator coverage

| Component                              | Planned delivery                                                                                                                                               |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Zsh                                    | Standard completion functions plus pinned `zsh-completions`, required helpers, managed initialization and general cursor-context bridge                        |
| Bash                                   | Compatible managed Bash plus pinned `bash-completion`; programmable-completion state, lazy loaders, completion options and fallback semantics                  |
| Fish                                   | Compatible managed Fish plus completion/helper functions; `complete -C` adapter; separate Fish document frontend in P4                                         |
| Tool-provided definitions              | Discover trusted installed definitions; select by actual tool identity/version; preserve provenance and dependency requirements                                |
| Runtime dependencies                   | Audit helper/shared-library closure, relocation, minimum OS/libc and executable permissions; ship required private dependencies                                |
| First authoritative validator batch    | GNU/BSD `ls`, `eza`, Homebrew, Pacman, Git, including installed extension commands where applicable                                                            |
| Broader validator batch                | curl, SSH, Docker/Compose, kubectl, ripgrep, fd, bat and remaining existing common-file-tool specifications where reliable grammar evidence can be established |
| Unknown tool/version/extension grammar | Native suggestions continue; validation returns Unknown for uncovered contexts; compatibility manifest explains supported coverage                             |
| Existing Rust providers                | Keep fixed-query fallbacks during migration; consolidate only after native behavior and latency parity are demonstrated                                        |
| Updates                                | Pinned revisions/checksums and reproducible build recipes; update PRs run compatibility, license and package tests before release                              |

Broader coverage is a finite tested compatibility matrix plus a reusable provider interface, not a claim of exhaustive validation for every CLI or future version. Each listed tool requires recorded coverage or a documented Unknown boundary for genuinely unavailable authoritative metadata.

### Editor lifecycle and UX

| Topic                   | Planned behavior                                                                                                                                                                                                                                                                   |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Target selection        | Active-document status shows workspace host, shell and script/interactive mode; per-document override, workspace profile and inferred default have documented precedence                                                                                                           |
| cwd                     | Workspace folder is an explicit assumed launch cwd unless configured/captured; do not claim the document directory is the runtime cwd                                                                                                                                              |
| Personal state          | My shell uses applicable trusted personal state after selecting that context; startup-file editing uses static entry-state analysis without automatic sourcing/reloading; managed defaults require no startup files; standalone scripts do not inherit unrelated interactive state |
| Syntax feedback         | Syntax errors on for all clients by default; sensible recovery ranges and parser/linter deduplication                                                                                                                                                                              |
| Environment scheduling  | Approximately 400 ms idle debounce; one pending job/document; coalesce invalidations; prioritize interactive requests over workspace sweeps                                                                                                                                        |
| Event loop              | Workers return completion events; validate full snapshot key on the main loop before cache publication                                                                                                                                                                             |
| Push diagnostics        | Merge syntax/linter/environment results for the current version; late environment completion cannot erase syntax findings                                                                                                                                                          |
| Pull diagnostics        | Stable result IDs and unchanged reports; request diagnostic refresh only where supported; test clients without refresh support                                                                                                                                                     |
| Semantic tokens         | Shared resolution drives command/alias/builtin/function classifications and invalid modifier; contribute theme mappings and language defaults while respecting explicit user settings                                                                                              |
| Token refresh           | Negotiate `workspace/semanticTokens/refresh`; send coalesced refresh requests when matching command analysis or environment/session generations change, including changes without an edit                                                                                          |
| No-refresh clients      | Invalidate cached tokens and serve current results on the next client token request; do not synthesize document edits. VS Code uses the negotiated LSP refresh path; document that other clients without refresh support cannot guarantee immediate recoloring                     |
| Warning styling         | Warning diagnostics work even when semantic highlighting is disabled; test light/dark/high-contrast themes; arbitrary themes are not promised identical foreground colors                                                                                                          |
| Typo actions            | Conservative nearest-candidate selection; explicit alternatives; version-checked edits replace original source ranges only; no fix-all/on-save route                                                                                                                               |
| Hover                   | Target/interpreter, executable or source identity, alias expansion, declaration and uncertainty; show only safe metadata by default                                                                                                                                                |
| Guarded dependencies    | Prove the relevant command's availability only in the dominated region; handle supported positive/negative checks, terminating branches and boolean operators                                                                                                                      |
| Refresh                 | Watch relevant executable/package/plugin/config inputs where practical; debounce events and use bounded polling/TTL elsewhere; explicit Refresh environment action                                                                                                                 |
| Environment mutation    | Installing a file and changing an inherited PATH are separate events; PATH changes require context/session recapture                                                                                                                                                               |
| Comparison presentation | Read-only report/panel; ordinary Problems remains attached to the selected target; explicit multi-target diagnostics, if enabled, carry target labels                                                                                                                              |

### Terminal, inventory and history boundaries

| Topic                      | Planned behavior                                                                                                                                                                                            |
| -------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Live terminal state        | Collect metadata at prompt boundaries with shipped hooks; separate terminal IDs, process start identities and generations                                                                                   |
| Attachment                 | Shucked terminal profile initializes hooks without user dotfile edits; existing sessions require explicit attachment; never inject probes through command-execution APIs into an active job                 |
| Transport                  | User-restricted local IPC with session authentication, bounded messages and validated schemas; normal Remote SSH uses remote-host IPC                                                                       |
| Captured state             | Relevant PATH/cwd/options, alias mappings and function names; arbitrary function bodies and unrelated environment values are excluded by default                                                            |
| Transient completion state | Prototype a bounded session-side query hook for custom completers; callbacks run only on explicit completion requests, never by replaying edited commands; inaccessible/opaque state is reported as partial |
| Startup changes            | Never reload edited shell configuration on keystrokes; reflect statically understood changes in document analysis and offer explicit shell-context restart/recapture for live behavior                      |
| Terminal exit/nested SSH   | Detach visibly; do not silently reuse old authoritative absence evidence or interpret nested remote state as the workspace host                                                                             |
| Target export              | Deterministic versioned data, integrity metadata, completeness, provenance and capture date; no executable payloads, arbitrary environment values, history or credentials                                   |
| Alias export               | Optional sanitized/simple command mappings only; unsupported or potentially sensitive expansion data is omitted and marked incomplete, rather than relying on a secret-detection guarantee                  |
| Captured authority         | An explicitly selected pinned snapshot describes its recorded target; age is visible; never claim it reflects current live state automatically                                                              |
| Offline comparison         | Reuse per-target resolver/validator evidence; available/missing/unknown/incompatible capabilities stay distinct                                                                                             |
| History defaults           | Both collection and existing-file reads disabled by default; session and existing-history opt-ins are separate and revocable                                                                                |
| History handling           | Honor shell exclusions/private modes; bounded memory-only index; no raw command logging/telemetry/export; clear retained state on disable/context disposal                                                  |
| Remote history             | Storage/index/query stay on the workspace host; only selected suggestion text reaches the editor over its existing connection                                                                               |
| Suggestion acceptance      | Insert text only; never execute, auto-accept, or silently change a selected execution target                                                                                                                |

## Parallel execution and merge order

Limit: lead plus three active sub-agents. Agents own disjoint files; shared schemas have one designated owner. Each implementation task produces code, focused behavior tests, interface notes, and review evidence.

| Wave | Agent A                                 | Agent B                             | Agent C                                      | Lead integration gate                                                                                   |
| ---- | --------------------------------------- | ----------------------------------- | -------------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| 0    | P0 context/resolver contracts           | P0 runtime/platform feasibility     | P0 editor/terminal prototypes                | Record major choices; freeze versioned contracts and fixture interfaces                                 |
| 1    | P1 environment service                  | P2 semantic command facts           | P3 workers/pack tooling                      | First exact host command lookup and native completion through common context                            |
| 2    | P5 scheduling/context/basic diagnostics | P4 Fish frontend                    | P3 shell adapters/runtime packs              | Working typo warning, alias-aware completion, isolated providers; integrate Fish after its grammar gate |
| 3    | P6 authoritative validators             | P7 guards/declarations/refresh      | P5 highlighting/hover/manual fixes           | Complete Must behavior and Should dependency/refresh behavior                                           |
| 4    | P8 terminal integration                 | P9 inventories/comparison           | P6 broader coverage and compatibility corpus | Live state and offline targets share resolver evidence                                                  |
| 5    | P10 history inline suggestions          | P11 installed-package/remote matrix | Independent review of integrated behavior    | All Could milestones complete; fix reviewed defects                                                     |
| 6    | Review-fix implementation               | Independent regression verification | Release/artifact validation                  | Final requirement ledger and platform evidence sign-off                                                 |

Critical dependencies: P0 → P1/P2 → P5; P0 → P3; P0 → P4 → Fish editor integration; P1/P2/P3 → P6; P6 → P9 comparisons; P8 → session-history integration in P10. Early prototypes use fake providers; shipped features must satisfy the final bundle and parity gates.

The lead owns shared-schema changes, integration sequencing, and cross-cutting review. An agent never serves as the sole reviewer of its own implementation. Review findings judged valid are assigned for fixes without asking the user about routine corrections.

## Acceptance and release evidence

| Scenario                                                                     | Required result                                                                                                                     |
| ---------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `asdjasijdasoijo`, complete workspace context                                | Debounced target-qualified warning and invalid-command classification                                                               |
| Same document, Portable                                                      | Host-absence warning disappears; parser diagnostics remain                                                                          |
| Function/source/builtin/executable/applicable alias                          | Same resolved identity across all editor consumers                                                                                  |
| `alias ls='eza --icons'`, applicable interactive context                     | Eza completions/validation with preserved injected arguments and original edit ranges                                               |
| Same alias only in interactive startup, standalone script                    | No inappropriate alias inheritance                                                                                                  |
| Alias same-line definitions, function bodies, unalias, quoting and recursion | Shell-specific behavior or explicit Unknown; no generic textual substitution                                                        |
| `brew abcsdasd` / `brew intsall`                                             | Invalidity only with complete installed command/extension evidence; manual typo action for supported typo                           |
| Unknown version/new plugin/partial completion response                       | No unjustified invalid-subcommand/flag warning                                                                                      |
| Incomplete quote/malformed `if`; malformed Fish constructs                   | Parser diagnostic in the correct dialect and range                                                                                  |
| `$cmd`, dynamic PATH/cwd, unresolved source, timeout/read error              | Unknown with no false absence warning                                                                                               |
| `command`, `builtin`, `env`, `exec`, `sudo`, `ssh`, container wrappers       | Correct modeled namespace/context; unsupported changed-host state stays Unknown                                                     |
| Tool exists outside actual PATH or only in private helper bundle             | Bare command is not falsely resolved                                                                                                |
| Optional command guarded by availability check                               | Suppress only where proof applies; unrelated commands still checked                                                                 |
| Generated/CI command declaration                                             | Declared provenance; separate unmet-target dependency feedback; no fabricated installed identity                                    |
| Install/remove/upgrade, config/plugin/PATH changes                           | Appropriate recapture/invalidation; all consumers converge without restarting editor                                                |
| Native provider finishes or alias/tool availability changes without edits    | Diagnostics and coloring refresh from the same generation; refresh-capable and no-refresh clients follow their documented contracts |
| Editing a startup file with later alias/function definitions                 | No automatic execution/reload; later/post-startup definitions cannot make earlier calls falsely resolved                            |
| Rapid edits/target switch/close during slow work                             | No stale diagnostics, tokens, suggestions or code actions; no orphan processes                                                      |
| Unicode, quoted paths, midword cursors, embedded shell                       | Correct encoding and source-mapped ranges for every consumer                                                                        |
| Empty HOME, absent host shells/completion frameworks, offline                | Bundled baseline works without .zshrc/.bashrc/Fish configuration; dynamic queries use installed target tools/databases              |
| Zsh/Bash/Fish documents                                                      | Independent syntax/call-fact fixtures, matching shell semantics and native candidates                                               |
| Remote SSH/WSL/container/reconnect                                           | Correct workspace-host executables/state; no local/remote cache leakage                                                             |
| Live terminal alias/function/PATH/cwd changes                                | Only linked contexts refresh; session termination revokes current-state claims                                                      |
| Offline imported Arch inventory on macOS                                     | Target results do not depend on local Brew or local completion subprocesses                                                         |
| Multi-target BSD/GNU tool differences                                        | Target-labelled capability report, with Unknown preserved where evidence is incomplete                                              |
| History disabled/enabled/revoked                                             | No reads while off; explicit scoped suggestions while on; caches cleared when revoked                                               |
| Semantic highlighting disabled/custom themes                                 | Standard warning diagnostics remain available; no assertion of guaranteed red foreground                                            |
| Untrusted workspace                                                          | Static intelligence remains usable; native code execution cannot be enabled by workspace configuration                              |

| Test layer                 | Location / execution                                                                                                                             |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| Semantic behavior          | Mirrored tests under `crates/shucked-semantic/tests`; fixtures for scope, alias parsing, guards, branch effects and source closure               |
| Resolver/provider behavior | New shared-crate tests; isolated filesystems/fake executables, deterministic snapshots, cancellation/timeout/output limits                       |
| LSP protocol               | `crates/shucked-lsp/tests`, `tests/lsp`; push/pull parity, refresh support/fallback, version races, encodings and fix-all exclusions             |
| VS Code                    | Extension unit tests plus Extension Development Host tests for status/picker, themes, terminal hooks, inline suggestions and Problems            |
| Packaging                  | `tests/packaging`, `tooling`; install actual VSIX in isolated hosts, missing user runtimes, offline startup, helper dependency/relocation checks |
| Remote                     | Real SSH fixture, WSL fixture and dev-container fixture; host identity, reconnect, permission/read failures and process cleanup                  |
| Security/privacy           | Protocol spoofing/malformed payloads, import bounds, untrusted settings, history exclusion and no secret/body leakage                            |
| Licensing/supply chain     | Pinned input checksums, per-file notices, source/build redistribution artifacts, SBOM and update verification                                    |

Initial performance targets, calibrated against P0 measurements: warm completion p95 under 50 ms; cached command lookup under 10 ms; edit notification handling p95 under 10 ms; ordinary environment warnings visible within 700 ms after the last edit including debounce. Slow native work is bounded/cancellable and cannot suppress fast syntax feedback. Any revised target must include measurements and explicit rationale; no unmeasured parity claim.

Use existing repository commands for Rust tests/Clippy/formatting and extension typecheck/lint/tests. Add dedicated tooling entries for provider, terminal, remote and installed-VSIX matrices. Do not run the large compatibility corpus merely for environment/UI changes.

## Major decision gates before implementation

These are concrete proposals to confirm at P0; the plan does not treat them as already approved. Routine implementation details and review fixes do not need renewed permission.

| Gate                               | Recommendation                                                                                                                                                             | Alternative / tradeoff                                                                                              | Consequence                                                                                                                               |
| ---------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| D1 — Native Windows delivery       | Prototype and package a private compatible Unix runtime/helper layer; retain zero-setup goal for Windows-local workspaces                                                  | WSL-only native intelligence is smaller but changes the Windows-local promise and requires explicit user acceptance | Validate process cancellation, filesystem translation, architecture, dependencies, licensing and package size before committing to parity |
| D2 — Fish scope                    | Include P4 Fish frontend so command recognition, syntax diagnostics, highlighting and edits use Fish semantics                                                             | Completion-only Fish support is less work but does not satisfy comparable document intelligence                     | Plan includes frontend work; full parity with every existing Bash/Zsh lint rule is a separate claim and is not implied                    |
| D3 — Alias/personal-state defaults | Workspace script by default; startup-file editing is static and source-ordered; selected My shell may initialize trusted on-disk startup code; live attachment is explicit | Importing interactive aliases into every local script is convenient but can predict the wrong runtime behavior      | Agree startup-loading/entry-state rules, show the selected context, and never execute buffers or auto-reload edited startup files         |
| D4 — Invalid-command appearance    | Theme-aware invalid classification plus warning squiggles                                                                                                                  | Forced foreground decorations give stronger uniformity but can conflict with themes/accessibility/user choices      | No promise that every theme colors the token identically unless decorations are selected                                                  |
| D5 — History and terminal capture  | Optional terminal profile/attach; history off, separate session/file opt-ins; memory-only history caches                                                                   | Automatic attachment/history ingestion increases convenience but expands collection and interaction                 | Ship useful completions without enabling any history collection                                                                           |
| D6 — Target authority and cwd      | Explicit assumed workspace cwd; visible pinned-snapshot age; declaration and installed-availability states remain separate                                                 | Guessing script-directory cwd or treating old snapshots as live produces stronger but unreliable assertions         | UI and diagnostics name assumptions/recorded target, and uncertain facts remain Unknown                                                   |

### Distribution gate

Reconcile current six Cargo release targets with the nine extension targets: macOS x64/ARM64; Linux GNU x64/ARM64/ARMHF; Alpine/musl x64/ARM64; Windows x64/ARM64. Recommended plan adds missing release/runtime/test coverage rather than silently dropping advertised targets. Native Windows runtime feasibility, especially ARM64, must be established; a change to this matrix is a user-visible decision. Model the actual selected interpreter's executable lookup, PATHEXT behavior where applicable, drive/path translation and cwd separately from the private completion engine. A shipped provider runtime does not prove that the user-selected script interpreter or target commands are installed; represent unavailable target interpreters explicitly.

Bundled components do not share one license. Track Zsh/per-file completion terms, Bash/bash-completion GPL terms, Fish's mixed license inventory, helper libraries and any compatibility runtime. Release only with the applicable notices and source/build materials for redistributed components. Keep clean-room diagnostic authoring separate from third-party completion assets.

## Independent planning review

| Review                          | Result                                                                                                                                                            |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| M/S/C coverage and dependencies | All ledger items have work packages, gates and acceptance scenarios                                                                                               |
| Startup execution boundary      | Corrected: buffers/source-graph analysis, trusted on-disk startup initialization, and attached-session capture are distinct; startup-file edits never auto-reload |
| Asynchronous coloring           | Corrected: explicit semantic-token refresh negotiation, no-edit change cases, and no-refresh-client limitations                                                   |
| Review recheck                  | Independent reviewer verified both corrections; no remaining necessary planning corrections                                                                       |
| Implementation review           | Separate agents review each integrated slice; valid findings are fixed and rechecked before release                                                               |

## Definition of done

- Every M/S/C ledger row links to merged implementation and passing acceptance evidence.
- Default workspace-host checks and Portable opt-in work without manual shell configuration.
- Commands, aliases, warnings, hovers and suggestions agree on identity and execution context.
- Each advertised package works from a clean installed VSIX, including runtime/helper dependencies.
- Fish is never silently analyzed as Bash; completion runtime choice never changes document semantics.
- Live terminal, offline inventories, deployment comparisons and optional history are shipped and tested.
- Independent review has been completed; agreed defects fixed and rechecked.
- Platform, validation-coverage, custom-completer and theme limitations are documented explicitly rather than reported as full parity.

## Primary references

| Subject                              | Source                                                                                      |
| ------------------------------------ | ------------------------------------------------------------------------------------------- |
| Zsh startup semantics                | https://zsh.sourceforge.io/Doc/Release/Files.html                                           |
| Zsh completion definitions/licensing | https://github.com/zsh-users/zsh-completions                                                |
| Bash programmable completion pack    | https://github.com/scop/bash-completion                                                     |
| Fish completion query API            | https://fishshell.com/docs/current/cmds/complete.html                                       |
| Fish runtime/platform requirements   | https://github.com/fish-shell/fish-shell/blob/master/README.rst                             |
| Fish licenses                        | https://github.com/fish-shell/fish-shell/blob/master/COPYING                                |
| Windows compatibility runtime        | https://www.msys2.org/docs/what-is-msys2/                                                   |
| VS Code semantic tokens/themes       | https://code.visualstudio.com/api/language-extensions/semantic-highlight-guide              |
| VS Code terminal integration         | https://code.visualstudio.com/docs/terminal/shell-integration                               |
| VS Code terminal API                 | https://code.visualstudio.com/api/references/vscode-api#TerminalShellIntegration            |
| LSP lifecycle and refresh            | https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/ |
