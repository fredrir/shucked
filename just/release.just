set shell := ["bash", "-uc"]
set default-list
set working-directory := '..'

release := 'cargo run -q -p shucked-tooling -- release'

# Harden .github/workflows/release.yml (--fix, --workflow)
check-security *args="":
    {{ release }} check-security {{ args }}

# Verify .release-please-config.json crate and python mappings
check-config:
    {{ release }} check-config

# Generate a CycloneDX SBOM
generate-sbom:
    {{ release }} generate-sbom
