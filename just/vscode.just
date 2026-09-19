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

# Run extension tests
test:
    {{ vscode }} test

# Lint extension sources
lint:
    {{ vscode }} lint
