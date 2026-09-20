use std::path::{Path, PathBuf};

use shucked_indexer::Indexer;
use shucked_parser::{ShellDialect, ShellProfile, parser::Parser};
use shucked_semantic::{SemanticBuildOptions, SemanticModel};

fn candidate(source: &str) -> Option<PathBuf> {
    let source = format!("#!/usr/bin/env bash\n{source}");
    let source = source.as_str();
    let profile = ShellProfile::native(ShellDialect::Bash);
    let parse = Parser::with_profile(source, profile.clone()).parse();
    assert!(!parse.is_err(), "{source}");
    let indexer = Indexer::new(source, &parse);
    let path = Path::new("/workspace/project/scripts/consumer.sh");
    let model = SemanticModel::build_with_options(
        &parse.file,
        source,
        &indexer,
        SemanticBuildOptions {
            source_path: Some(path),
            shell_profile: Some(profile),
            resolve_source_closure: false,
            ..Default::default()
        },
    );
    model.current_file_source_candidate(model.source_refs().last().unwrap(), path)
}

#[test]
fn resolves_source_paths_through_directory_assignments() {
    for source in [
        r#"SCRIPT_DIR="$(dirname -- "${BASH_SOURCE[0]}")"
source "${SCRIPT_DIR}/helper.sh""#,
        r#"SCRIPT_DIR="$(cd -L -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -L)"
source "${SCRIPT_DIR}/helper.sh""#,
        r#"SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "$SCRIPT_DIR/.." && pwd)"
LIB_DIR="${ROOT_DIR}/lib"
source "$LIB_DIR/helper.sh""#,
        r#"SCRIPT_DIR="$(dirname -- "${BASH_SOURCE[0]}")"
source "$(cd -- "${SCRIPT_DIR}/../lib" && pwd)/helper.sh""#,
    ] {
        let expected = if source.contains("lib") {
            "/workspace/project/lib/helper.sh"
        } else {
            "/workspace/project/scripts/helper.sh"
        };
        assert_eq!(candidate(source), Some(PathBuf::from(expected)), "{source}");
    }
}

#[test]
fn refuses_ambiguous_directory_assignments_and_substitutions() {
    for source in [
        r#"DIR="$(cd -- "$unknown" && pwd)"; source "$DIR/helper.sh""#,
        r#"DIR="$unknown$(dirname -- "${BASH_SOURCE[0]}")"; source "$DIR/helper.sh""#,
        r#"DIR="$(dirname -- "${BASH_SOURCE[0]}")"; unset DIR; source "$DIR/helper.sh""#,
        r#"DIR="$(dirname -- "${BASH_SOURCE[0]}")"; DIR+="/other"; source "$DIR/helper.sh""#,
        r#"DIR="$(dirname -- "${BASH_SOURCE[0]}")"; NEXT="$(cd -- $DIR && pwd)"; source "$NEXT/helper.sh""#,
        r#"DIR="$(cd -P -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"; source "$DIR/helper.sh""#,
        r#"DIR="$(dirname -- "${BASH_SOURCE[0]}")"; DIR="$runtime"; source "$DIR/helper.sh""#,
        r#"if test "$flag"; then DIR="$(dirname -- "${BASH_SOURCE[0]}")"; fi; source "$DIR/helper.sh""#,
        r#"DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")"; pwd)"; source "$DIR/helper.sh""#,
        r#"DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" || pwd)"; source "$DIR/helper.sh""#,
        r#"DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd && echo surprise)"; source "$DIR/helper.sh""#,
        r#"DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd > elsewhere)"; source "$DIR/helper.sh""#,
    ] {
        assert_eq!(candidate(source), None, "{source}");
    }
}

#[test]
fn bounds_recursively_derived_path_templates() {
    let mut source = String::from(r#"DIR="$(dirname -- "${BASH_SOURCE[0]}")""#);
    for _ in 0..300 {
        source.push_str("\nDIR=\"$(cd -- \"$DIR\" && pwd)\"");
    }
    source.push_str("\nsource \"$DIR/helper.sh\"");
    assert_eq!(candidate(&source), None);
}

#[test]
fn bounds_repeated_literal_concatenation() {
    let mut source = String::from("DIR=path");
    for _ in 0..40 {
        source.push_str("\nDIR=\"$DIR$DIR\"");
    }
    source.push_str("\nsource \"$DIR/helper.sh\"");
    assert_eq!(candidate(&source), None);
}
