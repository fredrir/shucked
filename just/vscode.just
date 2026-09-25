set shell := ["bash", "-uc"]
set default-list
set working-directory := '..'

vscode := 'cargo run -q -p shucked-tooling -- vscode'

# Compile the extension
compile:
    {{ vscode }} compile

# Package the extension into a .vsix
package:
    {{ vscode }} package

# [ --e2e, --vsix PATH, --build-vsix, -- PYTEST_ARGS ] -- Run extension tests
test *args="":
    {{ vscode }} test {{ args }}

# Lint extension sources
lint:
    {{ vscode }} lint
