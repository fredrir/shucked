# Remote shell intelligence acceptance

| Input | Value |
|---|---|
| Target | Actual SSH, container, or WSL host |
| Server | Absolute target-host `shucked` binary path |
| Target prerequisites | `/usr/bin/python3`, `/usr/bin/env`, `/bin/sh`, `uname` |
| Fixture | Temporary isolated target directory; removed after each run |
| Local fixture | Executable with a competing prefix; must never appear remotely |
| Checks | Command inventory, quoted paths, install refresh without edits, reconnect |
| Relative PATH | `--relative-path`; explicit launch cwd, independent of server cwd |
| SSH authentication | Caller-provided SSH configuration; strict host-key verification |
| Editor scope | LSP transport acceptance; VS Code UI acceptance runs separately |

```sh
uv run --project tests python tests/remote/acceptance.py \
  --command '["ssh","-T","fixture"]' --ssh \
  --binary /opt/shucked/shucked --name SSH

uv run --project tests python tests/remote/acceptance.py \
  --command '["podman","exec","-i","fixture"]' \
  --binary /opt/shucked/shucked --name container --relative-path

uv run --project tests python tests/remote/acceptance.py \
  --command '["wsl.exe","--distribution","Ubuntu","--exec"]' \
  --binary /opt/shucked/shucked --name WSL
```

| Executed host | Result |
|---|---|
| Linux ARM64 over SSH from macOS ARM64 | Passed; absolute and relative PATH |
| Linux ARM64 container over `podman exec` | Passed; absolute and relative PATH |
| WSL | Harness available; Windows host execution required |
