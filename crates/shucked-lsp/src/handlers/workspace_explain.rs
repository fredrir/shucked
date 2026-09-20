use std::collections::BTreeMap;

use lsp_types as types;

use crate::workspace_functions::{
    SourceResolutionReason, WorkspaceFunctionIndex, WorkspaceSourceDetails,
    WorkspaceVariableDetails,
};

const MAX_LINKS: usize = 8;

fn escaped(text: &str) -> String {
    text.chars()
        .flat_map(|ch| match ch {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' => vec!['\\', ch],
            '\n' | '\r' => vec![' '],
            _ => vec![ch],
        })
        .collect()
}

fn link(uri: &types::Url, line: Option<u32>) -> String {
    let label = uri
        .to_file_path()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| uri.to_string());
    let mut url = uri.clone();
    let label = match line {
        Some(line) => {
            url.set_fragment(Some(&format!("L{}", line + 1)));
            format!("{label}:{}", line + 1)
        }
        None => label,
    };
    format!("[{}](<{}>)", escaped(&label), url)
}

pub(crate) fn variable(
    details: &WorkspaceVariableDetails,
    index: &WorkspaceFunctionIndex,
) -> String {
    let mut text = format!("**Workspace variable {}**", escaped(&details.name));
    if details.definitions.is_empty() {
        text.push_str("\n\nNo known workspace assignment.");
    } else {
        text.push_str("\n\nAssignments:");
        text.push('\n');
        for location in details.definitions.iter().take(MAX_LINKS) {
            text.push_str(&format!(
                "\n- {}",
                link(&location.uri, Some(location.range.start.line))
            ));
        }
        if details.definitions.len() > MAX_LINKS {
            text.push_str(&format!(
                "\n- … and {} more assignments",
                details.definitions.len() - MAX_LINKS
            ));
        }
    }
    let mut consumers = BTreeMap::<&types::Url, (u32, usize)>::new();
    for location in &details.references {
        let entry = consumers
            .entry(&location.uri)
            .or_insert((location.range.start.line, 0));
        entry.0 = entry.0.min(location.range.start.line);
        entry.1 += 1;
    }
    if consumers.is_empty() {
        text.push_str("\n\nNo known workspace consumers.");
    } else {
        text.push_str("\n\nConsumed in:\n");
        for (uri, (line, count)) in consumers.iter().take(MAX_LINKS) {
            text.push_str(&format!(
                "\n- {} ({count} {})",
                link(uri, Some(*line)),
                if *count == 1 {
                    "reference"
                } else {
                    "references"
                }
            ));
        }
        if consumers.len() > MAX_LINKS {
            text.push_str(&format!(
                "\n- … and {} more files",
                consumers.len() - MAX_LINKS
            ));
        }
        text.push_str("\n\nUse **Go to References** for individual locations.");
    }
    if details.conditional {
        text.push_str("\n\nIncludes possible assignments and reads from conditional execution.");
    }
    if details.incomplete {
        text.push_str("\n\nReferences may include reads across unresolved source effects.");
        text.push_str(&format!(
            "\n\n**Incomplete results:** {}.",
            index
                .incomplete_reason()
                .unwrap_or_else(|| "some source effects could not be followed".into())
        ));
    }
    text
}

pub(crate) fn source(details: &WorkspaceSourceDetails, index: &WorkspaceFunctionIndex) -> String {
    let mut text = String::from("**Workspace source resolution**\n\n");
    text.push_str(match details.reason {
        SourceResolutionReason::Resolved => "Resolved source:",
        SourceResolutionReason::Missing => "Source file not found.",
        SourceResolutionReason::Dynamic => {
            "Unresolved: the path uses an unsupported runtime expression."
        }
        SourceResolutionReason::UnknownValue => {
            "Unresolved: the path depends on unknown or conflicting values."
        }
        SourceResolutionReason::AnalysisLimit => {
            "Unresolved: source analysis reached a file, depth, size, or work limit."
        }
        SourceResolutionReason::Ignored => "Source analysis disabled by the /dev/null directive.",
        SourceResolutionReason::Unreadable => {
            "Source file found, but its contents could not be read."
        }
    });
    if let Some(sequence) = &details.sequence {
        text.push_str("\n\nFiles matched by the source loop (in load order):\n");
        for path in sequence.iter().take(MAX_LINKS) {
            if let Ok(uri) = types::Url::from_file_path(path) {
                let uri = index.file(path).map_or(&uri, |file| file.editor_uri());
                text.push_str(&format!("\n- {}", link(uri, None)));
            }
        }
        if sequence.len() > MAX_LINKS {
            text.push_str(&format!(
                "\n- … and {} more files",
                sequence.len() - MAX_LINKS
            ));
        }
        if sequence.is_empty() {
            text.push_str("\nNo files matched.");
        }
    }
    if let Some(path) = &details.target
        && let Ok(uri) = types::Url::from_file_path(path)
    {
        let uri = index.file(path).map_or(&uri, |file| file.editor_uri());
        text.push_str(&format!("\n\n{}", link(uri, None)));
    } else if !details.candidates.is_empty() {
        text.push_str("\n\nSearched:\n");
        for path in details.candidates.iter().take(MAX_LINKS) {
            text.push_str(&format!("\n- {}", escaped(&path.display().to_string())));
        }
        if details.candidates.len() > MAX_LINKS {
            text.push_str(&format!(
                "\n- … and {} more paths",
                details.candidates.len() - MAX_LINKS
            ));
        }
    }
    if details.directive_span.is_some() {
        text.push_str("\n\nPath supplied by a source directive.");
    }
    if details.conditional {
        text.push_str("\n\nConditional source: execution is not guaranteed.");
    }
    if details.in_function {
        text.push_str("\n\nSource runs when its containing function is called.");
    }
    if let Some(reason) = index.incomplete_reason() {
        text.push_str(&format!(
            "\n\n**Incomplete workspace discovery:** {reason}."
        ));
    }
    text
}

pub(crate) fn function(
    resolution: &shucked_semantic::WorkspaceFunctionResolution,
    index: &WorkspaceFunctionIndex,
) -> String {
    let name = resolution
        .definitions
        .first()
        .map(|d| d.definition.name.as_str())
        .unwrap_or_default();
    let mut text = format!("**Workspace function {}**\n\nDefinitions:\n", escaped(name));
    for location in index
        .function_locations(&resolution.definitions)
        .iter()
        .take(MAX_LINKS)
    {
        text.push_str(&format!(
            "\n- {}",
            link(&location.uri, Some(location.range.start.line))
        ));
    }
    if !resolution.loaders.is_empty() {
        text.push_str("\n\nLoaded through:\n");
        for path in resolution.loaders.iter().take(MAX_LINKS) {
            if let Ok(uri) = types::Url::from_file_path(path) {
                text.push_str(&format!("\n- {}", link(&uri, None)));
            }
        }
    }
    let (references, incomplete) = index.function_references(&resolution.definitions);
    text.push_str(&format!(
        "\n\nWorkspace call sites: {}.\n",
        references.len()
    ));
    for location in references.iter().take(MAX_LINKS) {
        text.push_str(&format!(
            "\n- {}",
            link(&location.uri, Some(location.range.start.line))
        ));
    }
    if references.len() > MAX_LINKS {
        text.push_str(&format!(
            "\n- … and {} more calls",
            references.len() - MAX_LINKS
        ));
    }
    text.push_str("\n\nUse **Go to References** for individual locations.");
    if resolution.may_be_absent || resolution.definitions.len() > 1 {
        text.push_str("\n\nBinding depends on execution context.");
    }
    if incomplete || resolution.incomplete {
        text.push_str(&format!(
            "\n\n**Incomplete results:** {}. Known definitions and possible calls are shown.",
            index.incomplete_reason().unwrap_or_else(|| {
                "dynamic source or function effects could not be followed".into()
            })
        ));
    }
    text
}
