//! Static facts about zsh runtime configuration statements the editor
//! navigates: `fpath`/`FPATH` assignments, which decide where `autoload`
//! finds a function's file; `zle -N` widget registrations; and `bindkey`
//! widget references.
//!
//! Nothing is evaluated. Directories are rendered from literal words, `~`
//! and `$HOME`-anchored paths, the seeds the caller supplies (`HOME`,
//! `ZDOTDIR`, ...) and templates over scalar variables assigned earlier in the
//! same file; anything else marks the assignment incomplete.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;
use shucked_ast::{
    ArrayElem, AssignmentValue, Command, CompoundCommand, File, Name, Span, Stmt, StmtSeq, Word,
    static_word_text,
};

use crate::source_closure::{
    SourcePathTemplate, TemplatePart, assignment_path_template, expand_static_home_path,
    render_source_path_template, source_path_template_with_resolver, top_level_assignments,
};

/// One assignment to the function search path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZshFunctionPathAssignment {
    /// Span of the assignment.
    pub span: Span,
    /// Directories named statically, in search order.
    pub directories: Vec<PathBuf>,
    /// Whether the previous value stays in effect (`fpath+=(...)`, or a
    /// `$fpath` element in the new list).
    pub keeps_previous: bool,
    /// Whether an element could not be rendered to an absolute directory.
    pub incomplete: bool,
}

/// A `zle -N widget [function]` registration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZshWidgetRegistration {
    /// Span of the `zle` command.
    pub span: Span,
    /// Widget name.
    pub widget: Name,
    /// Span of the widget operand.
    pub widget_span: Span,
    /// Function implementing the widget (the widget name when omitted).
    pub function: Name,
    /// Span of the function operand when one was given.
    pub function_span: Option<Span>,
}

/// A `bindkey [...] key widget` reference to a widget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZshKeyBinding {
    /// Span of the `bindkey` command.
    pub span: Span,
    /// Widget name.
    pub widget: Name,
    /// Span of the widget operand.
    pub widget_span: Span,
}

/// Widget registrations and key bindings of one file, in source order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZshWidgetFacts {
    /// `zle -N` registrations.
    pub registrations: Vec<ZshWidgetRegistration>,
    /// `bindkey` references.
    pub bindings: Vec<ZshKeyBinding>,
}

/// Every `fpath`/`FPATH` assignment in `file`, in source order, including
/// those inside `if`, loop and brace bodies (function bodies run later and
/// are skipped).
///
/// `seeds` are variables known before the file runs, such as `HOME` and
/// `ZDOTDIR`; `home_dir` expands `~`.
pub fn zsh_function_path_assignments(
    file: &File,
    source: &str,
    source_path: &Path,
    home_dir: Option<&Path>,
    seeds: &[(&str, &Path)],
) -> Vec<ZshFunctionPathAssignment> {
    let mut templates = FxHashMap::<Name, SourcePathTemplate>::default();
    for (name, path) in seeds {
        templates.insert(
            Name::from(*name),
            SourcePathTemplate::Interpolated(vec![TemplatePart::Literal(
                path.to_string_lossy().replace('\\', "/"),
            )]),
        );
    }
    let mut assignments = Vec::new();
    walk_sequence(&file.body, &mut |stmt| {
        for assignment in top_level_assignments(stmt) {
            if assignment.target.subscript.is_some() {
                continue;
            }
            match assignment.target.name.as_str() {
                "fpath" | "FPATH" => {
                    assignments.push(function_path_assignment(
                        assignment,
                        source,
                        source_path,
                        home_dir,
                        &templates,
                    ));
                }
                _ if !assignment.append => {
                    if let Some(template) =
                        assignment_path_template(assignment, source, &templates, home_dir)
                    {
                        templates.insert(assignment.target.name.clone(), template);
                    }
                }
                _ => {}
            }
        }
    });
    assignments
}

/// Every `zle -N` registration and `bindkey` widget reference in `file`,
/// wherever it appears outside function bodies.
pub fn zsh_widget_facts(file: &File, source: &str) -> ZshWidgetFacts {
    let mut facts = ZshWidgetFacts::default();
    walk_sequence(&file.body, &mut |stmt| {
        let Command::Simple(command) = &stmt.command else {
            return;
        };
        let Some(name) = static_word_text(&command.name, source) else {
            return;
        };
        let args = command
            .args
            .iter()
            .map(|word| static_word_text(word, source).map(|text| (text.into_owned(), word.span)))
            .collect::<Option<Vec<_>>>();
        let Some(args) = args else {
            return;
        };
        match name.as_ref() {
            "zle" => {
                if let Some(registration) = widget_registration(command.span, &args) {
                    facts.registrations.push(registration);
                }
            }
            "bindkey" => {
                if let Some(binding) = key_binding(command.span, &args) {
                    facts.bindings.push(binding);
                }
            }
            _ => {}
        }
    });
    facts
}

fn walk_sequence<'a>(sequence: &'a StmtSeq, visit: &mut impl FnMut(&'a Stmt)) {
    for stmt in &sequence.stmts {
        walk_statement(stmt, visit);
    }
}

fn walk_statement<'a>(stmt: &'a Stmt, visit: &mut impl FnMut(&'a Stmt)) {
    visit(stmt);
    match &stmt.command {
        Command::Binary(binary) => {
            walk_statement(&binary.left, visit);
            walk_statement(&binary.right, visit);
        }
        Command::Compound(compound) => match compound {
            CompoundCommand::If(command) => {
                walk_sequence(&command.condition, visit);
                walk_sequence(&command.then_branch, visit);
                for (condition, body) in &command.elif_branches {
                    walk_sequence(condition, visit);
                    walk_sequence(body, visit);
                }
                if let Some(body) = &command.else_branch {
                    walk_sequence(body, visit);
                }
            }
            CompoundCommand::For(command) => walk_sequence(&command.body, visit),
            CompoundCommand::While(command) => {
                walk_sequence(&command.condition, visit);
                walk_sequence(&command.body, visit);
            }
            CompoundCommand::Until(command) => {
                walk_sequence(&command.condition, visit);
                walk_sequence(&command.body, visit);
            }
            CompoundCommand::BraceGroup(body) | CompoundCommand::Subshell(body) => {
                walk_sequence(body, visit);
            }
            _ => {}
        },
        _ => {}
    }
}

enum Element {
    Previous,
    Directory(PathBuf),
    Unknown,
}

fn function_path_assignment(
    assignment: &shucked_ast::Assignment,
    source: &str,
    source_path: &Path,
    home_dir: Option<&Path>,
    templates: &FxHashMap<Name, SourcePathTemplate>,
) -> ZshFunctionPathAssignment {
    let mut result = ZshFunctionPathAssignment {
        span: assignment.span,
        directories: Vec::new(),
        keeps_previous: assignment.append,
        incomplete: false,
    };
    let push = |element: Element, result: &mut ZshFunctionPathAssignment| match element {
        Element::Previous => result.keeps_previous = true,
        Element::Directory(directory) => {
            if !result.directories.contains(&directory) {
                result.directories.push(directory);
            }
        }
        Element::Unknown => result.incomplete = true,
    };
    match &assignment.value {
        AssignmentValue::Compound(array) => {
            for element in &array.elements {
                let ArrayElem::Sequential(value) = element else {
                    result.incomplete = true;
                    continue;
                };
                push(
                    render_element(value, source, source_path, home_dir, templates),
                    &mut result,
                );
            }
        }
        AssignmentValue::Scalar(word) => {
            // `FPATH=a:$FPATH` and `fpath+=dir`: a colon-separated scalar,
            // rendered segment by segment so `$FPATH` keeps the previous
            // value without hiding the directories beside it.
            let raw = word.span.slice(source);
            if raw.contains(':') {
                for segment in raw.split(':') {
                    let segment = segment.trim_matches(['"', '\'']);
                    if !segment.is_empty() {
                        push(
                            render_text_segment(segment, home_dir, templates),
                            &mut result,
                        );
                    }
                }
            } else {
                push(
                    render_element(word, source, source_path, home_dir, templates),
                    &mut result,
                );
            }
        }
    }
    result
}

/// Renders one colon-separated segment of a scalar path value: a
/// `$fpath`/`$FPATH` spelling, a `~`/`$HOME`-anchored path, `$NAME/tail` over a
/// literal seed or earlier assignment, or a literal absolute directory.
fn render_text_segment(
    segment: &str,
    home_dir: Option<&Path>,
    templates: &FxHashMap<Name, SourcePathTemplate>,
) -> Element {
    if matches!(
        segment,
        "$fpath" | "${fpath}" | "$fpath[@]" | "${fpath[@]}" | "$FPATH" | "${FPATH}"
    ) {
        return Element::Previous;
    }
    if let Some(rest) = segment.strip_prefix('$') {
        let (name, tail) = match rest.strip_prefix('{') {
            Some(braced) => match braced.split_once('}') {
                Some((name, tail)) => (name, tail),
                None => return Element::Unknown,
            },
            None => {
                let end = rest
                    .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                    .unwrap_or(rest.len());
                rest.split_at(end)
            }
        };
        let value = match templates.get(&Name::from(name)) {
            Some(SourcePathTemplate::Interpolated(parts)) => match parts.as_slice() {
                [TemplatePart::Literal(value)] => value.clone(),
                _ => return Element::Unknown,
            },
            None => return Element::Unknown,
        };
        let path = PathBuf::from(format!("{value}{tail}"));
        return if path.is_absolute() {
            Element::Directory(path)
        } else {
            Element::Unknown
        };
    }
    match expand_static_home_path(segment, home_dir) {
        Some(expanded) if Path::new(&expanded).is_absolute() => {
            Element::Directory(PathBuf::from(expanded))
        }
        _ => Element::Unknown,
    }
}

/// Whether the word is `$fpath`/`$FPATH` in one of its spellings.
fn word_mentions_previous_path(word: &Word, source: &str) -> bool {
    let raw = word.span.slice(source).trim_matches('"');
    matches!(
        raw,
        "$fpath" | "${fpath}" | "$fpath[@]" | "${fpath[@]}" | "${^fpath}" | "$FPATH" | "${FPATH}"
    ) || raw.contains("$fpath")
        || raw.contains("${fpath")
        || raw.contains("$FPATH")
        || raw.contains("${FPATH")
}

fn render_element(
    word: &Word,
    source: &str,
    source_path: &Path,
    home_dir: Option<&Path>,
    templates: &FxHashMap<Name, SourcePathTemplate>,
) -> Element {
    if word_mentions_previous_path(word, source) {
        return Element::Previous;
    }
    if let Some(text) = static_word_text(word, source) {
        return match expand_static_home_path(&text, home_dir) {
            Some(expanded) if Path::new(&expanded).is_absolute() => {
                Element::Directory(PathBuf::from(expanded))
            }
            _ => Element::Unknown,
        };
    }
    let template = source_path_template_with_resolver(word, source, false, true, |name, _| {
        templates.get(name).cloned()
    })
    .filter(|resolved| !resolved.ignored_root)
    .map(|resolved| resolved.template);
    match template.and_then(|template| render_source_path_template(&template, source_path)) {
        Some(path) if path.is_absolute() => Element::Directory(path),
        _ => Element::Unknown,
    }
}

fn widget_registration(span: Span, args: &[(String, Span)]) -> Option<ZshWidgetRegistration> {
    let registration = args.iter().position(|(arg, _)| arg == "-N")?;
    let operands = args[registration + 1..]
        .iter()
        .filter(|(arg, _)| !arg.starts_with('-'))
        .collect::<Vec<_>>();
    let (widget, function) = match operands.as_slice() {
        [widget] => (*widget, None),
        [widget, function, ..] => (*widget, Some(*function)),
        [] => return None,
    };
    if !is_zsh_function_name(&widget.0)
        || function.is_some_and(|(name, _)| !is_zsh_function_name(name))
    {
        return None;
    }
    Some(ZshWidgetRegistration {
        span,
        widget: Name::from(widget.0.as_str()),
        widget_span: widget.1,
        function: Name::from(function.map_or(widget.0.as_str(), |(name, _)| name.as_str())),
        function_span: function.map(|(_, span)| *span),
    })
}

fn key_binding(span: Span, args: &[(String, Span)]) -> Option<ZshKeyBinding> {
    let mut operands: Vec<&(String, Span)> = Vec::new();
    let mut index = 0;
    while let Some(operand) = args.get(index) {
        let arg = operand.0.as_str();
        index += 1;
        if arg == "-M" {
            // `-M keymap` selects the keymap the binding goes into.
            index += 1;
            continue;
        }
        if arg == "--" {
            operands.extend(args[index..].iter());
            break;
        }
        if let Some(flags) = arg.strip_prefix('-') {
            // String bindings, removals, listings and keymap operations name
            // no widget.
            if flags.contains(['s', 'r', 'l', 'L', 'd', 'D', 'p', 'N', 'A', 'm']) {
                return None;
            }
            continue;
        }
        operands.push(operand);
    }
    let [_, (widget, widget_span)] = operands.as_slice() else {
        return None;
    };
    if !is_zsh_function_name(widget) {
        return None;
    }
    Some(ZshKeyBinding {
        span,
        widget: Name::from(widget.as_str()),
        widget_span: *widget_span,
    })
}

fn is_zsh_function_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '+'])
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | ':' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shucked_parser::{ShellDialect, ShellProfile, parser::Parser};

    fn parse(source: &str) -> File {
        Parser::with_profile(source, ShellProfile::native(ShellDialect::Zsh))
            .parse()
            .file
    }

    #[test]
    fn function_path_assignments_render_home_zdotdir_and_earlier_variables() {
        let source = "ZSH_DIR=$HOME/.zsh\n\
                      fpath=($ZSH_DIR/functions ~/functions \"$ZDOTDIR/completions\" $fpath)\n\
                      fpath+=(/usr/local/share/zsh/site-functions)\n\
                      if [[ -d ~/extra ]]; then fpath=(~/extra $fpath); fi\n\
                      fpath=($unknown_dir $fpath)\n\
                      FPATH=/opt/f:$FPATH\n\
                      FPATH=$HOME/f:\"${ZDOTDIR}/g\"\n";
        let file = parse(source);
        let home = Path::new("/home/me");
        let zdotdir = Path::new("/home/me/.config/zsh");
        let assignments = zsh_function_path_assignments(
            &file,
            source,
            Path::new("/home/me/.config/zsh/.zshrc"),
            Some(home),
            &[("HOME", home), ("ZDOTDIR", zdotdir)],
        );
        let rendered = assignments
            .iter()
            .map(|assignment| {
                (
                    assignment
                        .directories
                        .iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect::<Vec<_>>(),
                    assignment.keeps_previous,
                    assignment.incomplete,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rendered,
            vec![
                (
                    vec![
                        "/home/me/.zsh/functions".to_owned(),
                        "/home/me/functions".to_owned(),
                        "/home/me/.config/zsh/completions".to_owned(),
                    ],
                    true,
                    false,
                ),
                (
                    vec!["/usr/local/share/zsh/site-functions".to_owned()],
                    true,
                    false,
                ),
                (vec!["/home/me/extra".to_owned()], true, false),
                (vec![], true, true),
                (vec!["/opt/f".to_owned()], true, false),
                (
                    vec!["/home/me/f".to_owned(), "/home/me/.config/zsh/g".to_owned()],
                    false,
                    false,
                ),
            ]
        );
    }

    #[test]
    fn widget_registrations_and_key_bindings_are_paired_by_name() {
        let source = "zle -N my-widget my_widget_fn\n\
                      zle -N self-insert\n\
                      bindkey '^X^E' my-widget\n\
                      bindkey -M viins '^R' history-incremental-search-backward\n\
                      bindkey -s '^G' 'git status\\n'\n\
                      bindkey -r '^Q'\n\
                      bindkey '^L'\n";
        let file = parse(source);
        let facts = zsh_widget_facts(&file, source);
        assert_eq!(
            facts
                .registrations
                .iter()
                .map(|r| (
                    r.widget.as_str(),
                    r.function.as_str(),
                    r.function_span.is_some()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("my-widget", "my_widget_fn", true),
                ("self-insert", "self-insert", false),
            ]
        );
        assert_eq!(
            facts
                .bindings
                .iter()
                .map(|b| b.widget.as_str())
                .collect::<Vec<_>>(),
            vec!["my-widget", "history-incremental-search-backward"]
        );
        let binding = &facts.bindings[0];
        assert_eq!(binding.widget_span.slice(source), "my-widget");
    }
}
