set shell := ["bash", "-uc"]
set default-list
set working-directory := '..'

fuzz := 'cargo run -q -p shucked-tooling -- fuzz'

# Seed toolchain, seeds, and directories (--ci, --cmin, --large-corpus)
init *args="":
    {{ fuzz }} init {{ args }}

# List registered fuzz targets
list:
    {{ fuzz }} list

# One deterministic pass over every PR-blocking target (-s, --sanitizer)
smoke *args="":
    {{ fuzz }} smoke {{ args }}

# Fuzz one target for 60s by default (-t SECS, -s SANITIZER)
run target *args="":
    {{ fuzz }} run {{ target }} {{ args }}

# Differential CLI fuzzing (-d DIALECT, -p PROFILE, -c COUNT, -s SEED, -w WORKERS)
cli *args="":
    {{ fuzz }} cli {{ args }}
