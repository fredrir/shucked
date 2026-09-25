//! Fixture-driven snapshot and invariant tests for semantic tokens.
//!
//! Snapshots live next to the fixtures under `snapshots/` and render one
//! decoded token per line as `line:col len type[mods] "text"`. Set
//! `INSTA_UPDATE=always` (or `UPDATE_SNAPSHOTS=1`) to rewrite them.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsp_types::Url;

use super::*;
use crate::edit::TextDocument;
use crate::handlers::commands::CommandService;
use crate::session::{Client, GlobalOptions, Session, Workspace, Workspaces};

/// Executables the fixtures rely on; anything else resolves as missing.
const FIXTURE_EXECUTABLES: &[&str] = &[
    "cat", "date", "diff", "git", "grep", "head", "ls", "sleep", "sort",
];

fn test_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/semantic_tokens")
}

fn fixture_source(name: &str) -> String {
    std::fs::read_to_string(test_dir().join("fixtures").join(name))
        .unwrap_or_else(|error| panic!("fixture {name} should be readable: {error}"))
}

/// A snapshot whose command resolution only sees a private, deterministic PATH.
fn fixture_snapshot(
    root: &Path,
    file_name: &str,
    language_id: &str,
    source: &str,
) -> DocumentSnapshot {
    let bin = root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for name in FIXTURE_EXECUTABLES {
        let path = bin.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    let (main_loop_sender, _main_loop_receiver) = crossbeam::channel::unbounded();
    let (client_sender, _client_receiver) = crossbeam::channel::unbounded();
    let client = Client::new(main_loop_sender, client_sender);
    let workspaces = Workspaces::new(vec![Workspace::default(
        Url::from_file_path(root).expect("temporary directory should convert to a file URL"),
    )]);
    let global = GlobalOptions::default().into_settings(client.clone());
    let mut session = Session::new(
        &lsp_types::ClientCapabilities::default(),
        PositionEncoding::UTF16,
        global,
        &workspaces,
        &client,
    )
    .expect("test session should initialize");

    let uri = Url::from_file_path(root.join(file_name)).unwrap();
    session.open_text_document(
        uri.clone(),
        TextDocument::new(source.to_owned(), 1).with_language_id(language_id),
    );
    let mut snapshot = session
        .take_snapshot(uri)
        .expect("test document should produce a snapshot");
    let service = CommandService::fixture(vec![bin]);
    snapshot.environment_generation = service.generation();
    snapshot.command_service = Arc::new(service);
    snapshot
}

fn tokens_for(root: &Path, fixture: &str, language_id: &str) -> (String, SemanticTokens) {
    let source = fixture_source(fixture);
    let snapshot = fixture_snapshot(root, fixture, language_id, &source);
    let tokens = semantic_tokens_full(snapshot)
        .expect("semantic tokens should not fail")
        .expect("the fixture dialect should produce tokens");
    (source, tokens)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecodedToken {
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
    modifiers: u32,
}

fn decode(tokens: &SemanticTokens) -> Vec<DecodedToken> {
    let mut decoded = Vec::with_capacity(tokens.data.len());
    let mut line = 0;
    let mut start = 0;
    for token in &tokens.data {
        line += token.delta_line;
        start = if token.delta_line == 0 {
            start + token.delta_start
        } else {
            token.delta_start
        };
        decoded.push(DecodedToken {
            line,
            start,
            length: token.length,
            token_type: token.token_type,
            modifiers: token.token_modifiers_bitset,
        });
    }
    decoded
}

/// Each line as UTF-16 code units, matching the encoding the tests request.
fn utf16_lines(source: &str) -> Vec<Vec<u16>> {
    source
        .split_inclusive('\n')
        .map(|line| line.trim_end_matches(['\n', '\r']).encode_utf16().collect())
        .collect()
}

fn modifier_names(bits: u32) -> String {
    SUPPORTED_TOKEN_MODIFIERS
        .iter()
        .enumerate()
        .filter(|(index, _)| bits & (1 << index) != 0)
        .map(|(_, modifier)| modifier.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn render(source: &str, tokens: &[DecodedToken]) -> String {
    let lines = utf16_lines(source);
    let mut rendered = String::new();
    for token in tokens {
        let line = &lines[token.line as usize];
        let text = String::from_utf16_lossy(
            &line[token.start as usize..(token.start + token.length) as usize],
        );
        let type_name = SUPPORTED_TOKEN_TYPES[token.token_type as usize].as_str();
        let modifiers = modifier_names(token.modifiers);
        rendered.push_str(&format!(
            "{}:{} {} {}{} {:?}\n",
            token.line,
            token.start,
            token.length,
            type_name,
            if modifiers.is_empty() {
                String::new()
            } else {
                format!("[{modifiers}]")
            },
            text
        ));
    }
    rendered
}

fn assert_snapshot(name: &str, actual: &str) {
    let path = test_dir().join("snapshots").join(format!("{name}.snap"));
    let update = std::env::var("INSTA_UPDATE").is_ok_and(|value| value == "always")
        || std::env::var_os("UPDATE_SNAPSHOTS").is_some();
    if update {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "snapshot {} is missing; run with INSTA_UPDATE=always to create it",
            path.display()
        )
    });
    if expected != actual {
        let first_difference = expected
            .lines()
            .zip(actual.lines())
            .position(|(expected, actual)| expected != actual)
            .unwrap_or_else(|| expected.lines().count().min(actual.lines().count()));
        panic!(
            "snapshot {} differs from the rendered tokens (first difference at line {}):\n--- expected\n{}\n--- actual\n{}\nrun with INSTA_UPDATE=always to accept",
            path.display(),
            first_difference + 1,
            expected
                .lines()
                .skip(first_difference)
                .take(8)
                .collect::<Vec<_>>()
                .join("\n"),
            actual
                .lines()
                .skip(first_difference)
                .take(8)
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

const FIXTURES: &[(&str, &str)] = &[
    ("bash.sh", "shellscript"),
    ("zsh.zsh", "zsh"),
    ("fish.fish", "fish"),
];

#[test]
fn bash_fixture_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (source, tokens) = tokens_for(root.path(), "bash.sh", "shellscript");
    assert_snapshot("bash", &render(&source, &decode(&tokens)));
}

#[test]
fn zsh_fixture_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (source, tokens) = tokens_for(root.path(), "zsh.zsh", "zsh");
    assert_snapshot("zsh", &render(&source, &decode(&tokens)));
}

#[test]
fn fish_fixture_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (source, tokens) = tokens_for(root.path(), "fish.fish", "fish");
    assert_snapshot("fish", &render(&source, &decode(&tokens)));
}

#[test]
fn tokens_are_sorted_disjoint_and_within_line_bounds() {
    let root = tempfile::tempdir().unwrap();
    for (fixture, language_id) in FIXTURES {
        let (source, tokens) = tokens_for(root.path(), fixture, language_id);
        let decoded = decode(&tokens);
        let lines = utf16_lines(&source);
        assert!(!decoded.is_empty(), "{fixture} should produce tokens");
        let mut previous: Option<DecodedToken> = None;
        for token in decoded {
            assert!(token.length > 0, "{fixture}: empty token {token:?}");
            assert!(
                (token.line as usize) < lines.len(),
                "{fixture}: token past the last line {token:?}"
            );
            let line_length = lines[token.line as usize].len() as u32;
            assert!(
                token.start + token.length <= line_length,
                "{fixture}: token exceeds its line ({line_length} units) {token:?}"
            );
            assert!(
                (token.token_type as usize) < SUPPORTED_TOKEN_TYPES.len(),
                "{fixture}: token type outside the legend {token:?}"
            );
            assert!(
                token.modifiers < 1 << SUPPORTED_TOKEN_MODIFIERS.len(),
                "{fixture}: modifier outside the legend {token:?}"
            );
            if let Some(previous) = previous {
                assert!(
                    (previous.line, previous.start) < (token.line, token.start),
                    "{fixture}: tokens out of order {previous:?} then {token:?}"
                );
                if previous.line == token.line {
                    assert!(
                        previous.start + previous.length <= token.start,
                        "{fixture}: overlapping tokens {previous:?} and {token:?}"
                    );
                }
            }
            previous = Some(token);
        }
    }
}

#[test]
fn tokens_are_deterministic_across_runs() {
    for (fixture, language_id) in FIXTURES {
        let first_root = tempfile::tempdir().unwrap();
        let second_root = tempfile::tempdir().unwrap();
        let (_, first) = tokens_for(first_root.path(), fixture, language_id);
        let (_, second) = tokens_for(second_root.path(), fixture, language_id);
        assert_eq!(
            first.data, second.data,
            "{fixture} tokens changed between runs"
        );
    }
}

#[test]
fn literal_text_inside_strings_is_the_only_string_token() {
    let root = tempfile::tempdir().unwrap();
    let source = "echo \"a $(date) ${x:-d} $((1)) $y b\"\n";
    let snapshot = fixture_snapshot(root.path(), "strings.sh", "shellscript", source);
    let tokens = semantic_tokens_full(snapshot).unwrap().unwrap();
    let rendered = render(source, &decode(&tokens));
    let strings = rendered
        .lines()
        .filter(|line| line.contains(" string "))
        .collect::<Vec<_>>();
    assert_eq!(
        strings,
        vec![
            "0:5 3 string \"\\\"a \"",
            "0:15 1 string \" \"",
            "0:23 1 string \" \"",
            "0:30 1 string \" \"",
            "0:33 3 string \" b\\\"\"",
        ],
        "{rendered}"
    );
    assert!(rendered.contains("operator \"$(\""), "{rendered}");
    assert!(rendered.contains("operator \":-\""), "{rendered}");
    assert!(rendered.contains("shellCommand \"date\""), "{rendered}");
}

#[test]
fn keywords_in_gaps_are_not_found_inside_comments() {
    let root = tempfile::tempdir().unwrap();
    let source = "if true; then\n  echo a\n  # not an else here\nelse\n  echo b\nfi\n";
    let snapshot = fixture_snapshot(root.path(), "gaps.sh", "shellscript", source);
    let tokens = semantic_tokens_full(snapshot).unwrap().unwrap();
    let rendered = render(source, &decode(&tokens));
    assert!(rendered.contains("3:0 4 keyword \"else\""), "{rendered}");
    assert!(rendered.contains("2:2 18 comment"), "{rendered}");
    assert_eq!(
        rendered.matches("keyword \"else\"").count(),
        1,
        "{rendered}"
    );
}

#[test]
fn existing_expectations_for_keywords_functions_and_arithmetic_still_hold() {
    let root = tempfile::tempdir().unwrap();
    let source = "greet() {\n  local name=$1\n  echo \"hello $name\"\n}\ngreet \"world\"\nval=$(( 40 + 2 ))\n";
    let snapshot = fixture_snapshot(root.path(), "legacy.sh", "shellscript", source);
    let tokens = semantic_tokens_full(snapshot).unwrap().unwrap();
    let rendered = render(source, &decode(&tokens));
    assert!(
        rendered.contains("0:0 5 function[declaration,definition] \"greet\""),
        "{rendered}"
    );
    assert!(rendered.contains("1:2 5 keyword \"local\""), "{rendered}");
    assert!(
        rendered.contains("1:8 4 variable[declaration] \"name\""),
        "{rendered}"
    );
    assert!(rendered.contains("1:13 2 parameter \"$1\""), "{rendered}");
    assert!(rendered.contains("2:14 5 variable \"$name\""), "{rendered}");
    assert!(rendered.contains("4:0 5 function \"greet\""), "{rendered}");
    assert!(rendered.contains("number \"40\""), "{rendered}");
    assert!(rendered.contains("number \"2\""), "{rendered}");
    assert!(rendered.contains("operator \"+\""), "{rendered}");
}
