use super::*;
use shucked_ast::{PatternPart, ZshGlobSegment};

pub const MAX_DIRECTORY_ENTRIES: usize = 4096;
const MAX_EXPANSIONS: usize = 128;

#[derive(Debug, Clone)]
pub(crate) struct LoopWord {
    parts: Vec<LoopPart>,
    null_glob: bool,
}

#[derive(Debug, Clone)]
enum LoopPart {
    Pattern(String),
    Value(SourcePathTemplate),
}

impl LoopWord {
    pub(crate) fn project(word: &Word, source: &str, bash: bool, zsh: bool) -> Option<Self> {
        let mut result = Self {
            parts: Vec::new(),
            null_glob: false,
        };
        result.word(word, source, bash, zsh)?;
        if zsh
            && let Some(LoopPart::Pattern(pattern)) = result.parts.last_mut()
            && pattern.ends_with("(N)")
        {
            pattern.truncate(pattern.len() - 3);
            result.null_glob = true;
        }
        if result.parts.iter().any(|part| matches!(part, LoopPart::Pattern(text) if text.contains(['(', ')', '^', '~', '#', '|']))) { return None; }
        (result.parts.len() <= MAX_SOURCE_PATH_TEMPLATE_PARTS).then_some(result)
    }

    fn word(&mut self, word: &Word, source: &str, bash: bool, zsh: bool) -> Option<()> {
        for part in &word.parts {
            match &part.kind {
                WordPart::Literal(text) => {
                    // Escaped metacharacters must never become active patterns.
                    if part.span.slice(source).contains('\\') {
                        return None;
                    }
                    self.parts
                        .push(LoopPart::Pattern(text.as_str(source, part.span).to_owned()));
                }
                WordPart::ZshQualifiedGlob(glob) if zsh => {
                    if let Some(qualifiers) = &glob.qualifiers {
                        if qualifiers.span.slice(source) != "(N)" {
                            return None;
                        }
                        self.null_glob = true;
                    }
                    for segment in &glob.segments {
                        let ZshGlobSegment::Pattern(pattern) = segment else {
                            return None;
                        };
                        for part in &pattern.parts {
                            match &part.kind {
                                PatternPart::Word(word) => self.word(word, source, bash, zsh)?,
                                PatternPart::Literal(text) => self.parts.push(LoopPart::Pattern(
                                    text.as_str(source, part.span).to_owned(),
                                )),
                                PatternPart::AnyString => {
                                    self.parts.push(LoopPart::Pattern("*".into()))
                                }
                                PatternPart::AnyChar => {
                                    self.parts.push(LoopPart::Pattern("?".into()))
                                }
                                PatternPart::CharClass(_) => self
                                    .parts
                                    .push(LoopPart::Pattern(part.span.slice(source).to_owned())),
                                _ => return None,
                            }
                        }
                    }
                }
                _ => {
                    let word = Word {
                        parts: vec![part.clone()],
                        span: part.span,
                        brace_syntax: Vec::new(),
                    };
                    self.parts.push(LoopPart::Value(source_path_expression(
                        &word, source, bash, zsh,
                    )?));
                }
            }
        }
        Some(())
    }

    pub(super) fn expand(
        &self,
        from: &Path,
        values: &FxHashMap<Name, String>,
        args: &[Option<compact_str::CompactString>],
        provider: &dyn SourcePathFileProvider,
        result: &mut ResolvedSourcePaths,
    ) -> Option<Vec<PathBuf>> {
        let mut pattern = String::new();
        for part in &self.parts {
            match part {
                LoopPart::Pattern(text) => pattern.push_str(text),
                LoopPart::Value(template) => pattern.push_str(&globset::escape(
                    &evaluate_expression(template, from, values, args)?,
                )),
            }
        }
        if pattern.len() > MAX_SOURCE_PATH_TEMPLATE_LITERAL_BYTES {
            result.incomplete = true;
            return None;
        }
        let patterns = expand_braces(&pattern, &mut result.incomplete)?;
        let mut paths = Vec::new();
        for pattern in patterns {
            let (directory, basename) = pattern.rsplit_once('/')?;
            let directory = literal_directory(directory)?;
            if !directory.is_absolute() {
                return None;
            }
            let matcher = globset::GlobBuilder::new(basename)
                .literal_separator(true)
                .backslash_escape(true)
                .build()
                .ok()?
                .compile_matcher();
            result.dependencies.insert(directory.clone());
            let Some(entries) = provider.directory_entries(&directory) else {
                result.incomplete = true;
                return None;
            };
            let mut matches = entries
                .into_iter()
                .filter(|path| {
                    path.file_name().is_some_and(|name| {
                        // Shell globs exclude leading dots unless explicitly requested.
                        (basename.starts_with('.') || !name.to_string_lossy().starts_with('.'))
                            && matcher.is_match(name)
                            && provider.is_file(path)
                    })
                })
                .take(MAX_EXPANSIONS + 1)
                .collect::<Vec<_>>();
            matches.sort();
            if matches.is_empty() && !self.null_glob {
                return None;
            }
            paths.extend(matches);
            if paths.len() > MAX_EXPANSIONS || provider.is_cancelled() {
                result.incomplete = true;
                return None;
            }
        }
        Some(paths)
    }
}

fn literal_directory(pattern: &str) -> Option<PathBuf> {
    let mut text = String::new();
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => text.push(chars.next()?),
            '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '~' | '^' | '#' | '|' => return None,
            _ => text.push(ch),
        }
    }
    Some(PathBuf::from(if text.is_empty() {
        "/".to_owned()
    } else {
        text
    }))
}

fn expand_braces(pattern: &str, incomplete: &mut bool) -> Option<Vec<String>> {
    let mut pending = vec![pattern.to_owned()];
    let mut expanded = Vec::new();
    while let Some(pattern) = pending.pop() {
        let mut escaped = false;
        let mut open = None;
        for (i, ch) in pattern.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == '{' {
                open = Some(i);
                break;
            }
        }
        let Some(start) = open else {
            expanded.push(pattern);
            continue;
        };
        let end = start + pattern[start..].find('}')?;
        let body = &pattern[start + 1..end];
        if body.contains(['{', '\\']) || !body.contains(',') {
            return None;
        }
        for alternative in body.split(',').rev() {
            pending.push(format!(
                "{}{alternative}{}",
                &pattern[..start],
                &pattern[end + 1..]
            ));
        }
        if pending.len() + expanded.len() > MAX_EXPANSIONS {
            *incomplete = true;
            return None;
        }
    }
    Some(expanded)
}

pub(super) fn directory_entries(path: &Path) -> Option<Vec<PathBuf>> {
    let entries = fs::read_dir(path)
        .ok()?
        .take(MAX_DIRECTORY_ENTRIES + 1)
        .map(|entry| entry.ok().map(|entry| entry.path()))
        .collect::<Option<Vec<_>>>()?;
    (entries.len() <= MAX_DIRECTORY_ENTRIES).then_some(entries)
}
