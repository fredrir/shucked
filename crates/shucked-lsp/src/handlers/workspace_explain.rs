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
        if let Some(reason) = index.incomplete_reason() {
            text.push_str(&format!("\n\n**Incomplete results:** {reason}."));
        }
        if !details.unfollowed_sources.is_empty() {
            text.push_str(
                "\n\n**Possible hidden reads:** these source operations run while the \
                 value is visible, but their target could not be inspected:\n",
            );
            for source in details.unfollowed_sources.iter().take(MAX_LINKS) {
                text.push_str(&format!(
                    "\n- {} `{}` ({})",
                    link(&source.location.uri, Some(source.location.range.start.line)),
                    source.text,
                    source.reason_text()
                ));
            }
            if details.unfollowed_sources.len() > MAX_LINKS {
                text.push_str(&format!(
                    "\n- … and {} more",
                    details.unfollowed_sources.len() - MAX_LINKS
                ));
            }
        } else if index.incomplete_reason().is_none() {
            text.push_str("\n\n**Incomplete results:** some source effects could not be followed.");
        }
    }
    text
}

/// Notice shown when a workspace reference query could not follow every
/// source operation that may read the selected value.
///
/// Returns a stable key identifying the situation (independent of which
/// variable was selected) together with the message text, so the client can
/// show it once and log later repetitions.
pub(crate) fn incomplete_references_notice(
    details: &WorkspaceVariableDetails,
    index: &WorkspaceFunctionIndex,
) -> (String, String) {
    const MAX_SITES: usize = 3;
    let mut message = format!("References to `{}` may be incomplete", details.name);
    let mut key = String::from("workspace-references-incomplete");
    if let Some(reason) = index.incomplete_reason() {
        message.push_str(&format!(": {reason}"));
        key.push_str(&format!(":{reason}"));
    }
    if !details.unfollowed_sources.is_empty() {
        message.push_str(if index.incomplete_reason().is_some() {
            "; "
        } else {
            ": "
        });
        let count = details.unfollowed_sources.len();
        message.push_str(&format!(
            "{count} source operation{} could not be followed while the value is visible",
            if count == 1 { "" } else { "s" }
        ));
        let mut sites = Vec::new();
        for source in details.unfollowed_sources.iter().take(MAX_SITES) {
            let file = source
                .location
                .uri
                .to_file_path()
                .ok()
                .and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| source.location.uri.to_string());
            sites.push(format!(
                "{file}:{} `{}` ({})",
                source.location.range.start.line + 1,
                source.text,
                source.reason_text()
            ));
            key.push_str(&format!(
                ":{}:{}",
                source.location.uri, source.location.range.start.line
            ));
        }
        message.push_str(&format!(" ({}", sites.join(", ")));
        if count > MAX_SITES {
            message.push_str(&format!(", … {} more", count - MAX_SITES));
        }
        message.push(')');
    } else if index.incomplete_reason().is_none() {
        message.push_str(": some source effects could not be followed");
    }
    message.push('.');
    (key, message)
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
