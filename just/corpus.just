set shell := ["bash", "-uc"]
set default-list
set working-directory := ".."

corpus := "cargo run -q -p shucked-tooling -- corpus"

# [--clone, --dry-run, --corpus-dir] Fetch fixtures
download *args="":
    {{ corpus }} download {{ args }}

# [--rules, --sample-percent] Compare vs ShellCheck
test *args="":
    {{ corpus }} test {{ args }}

# [--log, --output, --open] Generate HTML report
report *args="":
    {{ corpus }} report {{ args }}

# [--input, --output] Compact log
compact-log *args="":
    {{ corpus }} compact-log {{ args }}