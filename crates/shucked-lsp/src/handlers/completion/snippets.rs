//! Editable shell structures for clients that understand LSP snippets.
use lsp_types as types;

use crate::session::DocumentSnapshot;

pub(in crate::handlers) fn apply(snapshot: &DocumentSnapshot, items: &mut [types::CompletionItem]) {
    let capabilities = snapshot.resolved_client_capabilities();
    if !capabilities.completion_snippets {
        return;
    }
    let dialect = crate::handlers::commands::dialect(snapshot);
    for item in items {
        if item.kind != Some(types::CompletionItemKind::KEYWORD) {
            continue;
        }
        let Some(body) = body(dialect, &item.label) else {
            continue;
        };
        let Some(edit) = &mut item.text_edit else {
            continue;
        };
        match edit {
            types::CompletionTextEdit::Edit(edit) => edit.new_text = body.into(),
            types::CompletionTextEdit::InsertAndReplace(edit) => edit.new_text = body.into(),
        }
        item.kind = Some(types::CompletionItemKind::SNIPPET);
        item.sort_text = Some(format!("0:{}", item.label));
        item.insert_text_format = Some(types::InsertTextFormat::SNIPPET);
        item.insert_text_mode = capabilities
            .completion_adjust_indentation
            .then_some(types::InsertTextMode::ADJUST_INDENTATION);
        item.detail = Some(format!("{dialect} {} block", item.label));
    }
}

fn body(dialect: &str, keyword: &str) -> Option<&'static str> {
    if dialect == "fish" {
        return Some(match keyword {
            "if" => "if ${1:condition}\n\t${2:command}\nend\n$0",
            "else" => "else\n\t${1:command}\n$0",
            "for" => "for ${1:item} in ${2:items}\n\t${3:command}\nend\n$0",
            "while" => "while ${1:condition}\n\t${2:command}\nend\n$0",
            "function" => "function ${1:name}\n\t${2:command}\nend\n$0",
            "begin" => "begin\n\t${1:command}\nend\n$0",
            "switch" => "switch ${1:value}\n\tcase ${2:pattern}\n\t\t${3:command}\nend\n$0",
            "case" => "case ${1:pattern}\n\t${2:command}\n$0",
            _ => return None,
        });
    }
    Some(match keyword {
        "if" => "if ${1:condition}; then\n\t${2::}\nfi\n$0",
        "elif" => "elif ${1:condition}; then\n\t${2::}\n$0",
        "else" => "else\n\t${1::}\n$0",
        "for" => "for ${1:item} in ${2:items}; do\n\t${3::}\ndone\n$0",
        "while" => "while ${1:condition}; do\n\t${2::}\ndone\n$0",
        "until" => "until ${1:condition}; do\n\t${2::}\ndone\n$0",
        "case" => "case \"${1:word}\" in\n\t${2:pattern})\n\t\t${3::}\n\t\t;;\nesac\n$0",
        "function" if dialect == "sh" => "${1:name}() {\n\t${2::}\n}\n$0",
        "function" => "function ${1:name}() {\n\t${2::}\n}\n$0",
        "select" if dialect != "sh" => "select ${1:item} in ${2:items}; do\n\t${3::}\ndone\n$0",
        _ => return None,
    })
}

#[cfg(test)]
#[path = "../../../tests/completion/snippets.rs"]
mod tests;
