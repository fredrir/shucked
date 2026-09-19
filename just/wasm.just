set shell := ["bash", "-uc"]
set default-list
set working-directory := '..'

tooling := 'cargo run -q -p shucked-tooling --'

# Build shucked-wasm
build *args="":
    {{ tooling }} build --wasm {{ args }}

# Test shucked-wasm
test *args="":
    {{ tooling }} test --wasm {{ args }}
