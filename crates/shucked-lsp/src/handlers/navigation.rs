//! Navigation targets shared by the definition, declaration and
//! implementation requests: `source` operands, executable scripts, and
//! declaration sites.

use std::io::Read;
use std::path::Path;

use lsp_types as types;

use crate::session::DocumentSnapshot;
use crate::workspace_functions::WorkspaceFunctionIndex;

/// Location at the start of `path`, using the editor's URI for open buffers.
pub(crate) fn file_start_location(
    index: &WorkspaceFunctionIndex,
    path: &Path,
) -> Option<types::Location> {
    let uri = match index.file(path) {
        Some(file) => file.editor_uri().clone(),
        None => types::Url::from_file_path(path).ok()?,
    };
    Some(types::Location {
        uri,
        range: types::Range::default(),
    })
}

/// Files loaded by the `source`/`.` operand under `offset`, when the cursor is
/// on the operand itself (or on its `source=` directive) and the operation
/// resolved to at least one file.
pub(crate) fn source_operand_locations(
    index: &WorkspaceFunctionIndex,
    path: &Path,
    offset: usize,
) -> Option<Vec<types::Location>> {
    let details = index.source_details(path, offset)?;
    let within =
        |span: shucked_ast::Span| span.start.offset() <= offset && offset < span.end.offset();
    if !(within(details.path_span) || details.directive_span.is_some_and(within)) {
        return None;
    }
    let targets = match (&details.sequence, &details.target) {
        (Some(sequence), _) => sequence
            .iter()
            .map(|path| path.as_path())
            .collect::<Vec<_>>(),
        (None, Some(target)) => vec![target.as_path()],
        (None, None) => Vec::new(),
    };
    let locations = targets
        .into_iter()
        .filter_map(|target| file_start_location(index, target))
        .collect::<Vec<_>>();
    (!locations.is_empty()).then_some(locations)
}

/// The script implementing the external command whose name is under `offset`.
///
/// Compiled executables have nothing to show, so only files that start with a
/// shebang are offered.
pub(crate) fn command_script_location(
    snapshot: &DocumentSnapshot,
    offset: usize,
) -> Option<types::Location> {
    let commands = snapshot.command_service.analysis(snapshot);
    let (_, resolution) = commands.sites.iter().find(|(site, _)| {
        let span = site.name_span();
        span.start.offset() <= offset && offset < span.end.offset()
    })?;
    let shucked_command::CommandResolution::Resolved(command) = resolution else {
        return None;
    };
    if command.kind != shucked_command::CommandKind::Executable {
        return None;
    }
    let identity = command.executable.as_ref()?;
    if !starts_with_shebang(&identity.path) {
        return None;
    }
    Some(types::Location {
        uri: types::Url::from_file_path(&identity.path).ok()?,
        range: types::Range::default(),
    })
}

fn starts_with_shebang(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = [0u8; 2];
    matches!(file.read(&mut head), Ok(2)) && &head == b"#!"
}

/// Collapse locations into the LSP response shape.
pub(crate) fn response(locations: Vec<types::Location>) -> Option<types::GotoDefinitionResponse> {
    match locations.as_slice() {
        [] => None,
        [location] => Some(types::GotoDefinitionResponse::Scalar(location.clone())),
        _ => Some(types::GotoDefinitionResponse::Array(locations)),
    }
}
