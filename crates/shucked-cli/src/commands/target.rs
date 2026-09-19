use std::io::{Read, Write};
use std::path::Path;
use anyhow::{Context, Result};
use shucked_command::{CommandSite, ExecutionContext, ShellDialect, TargetInventory};
use crate::{ExitStatus, args::TargetCommand};

pub(crate) fn run(command: TargetCommand) -> Result<ExitStatus> {
    match command {
        TargetCommand::Capture { label, shell, output, cwd } => {
            let context = ExecutionContext {
                target_id: label.clone(), cwd: Some(cwd.unwrap_or(std::env::current_dir()?)), cwd_known: true,
                dialect: match shell.as_str() { "zsh" => ShellDialect::Zsh, "fish" => ShellDialect::Fish, "sh" => ShellDialect::Posix, "ksh" => ShellDialect::Mksh, _ => ShellDialect::Bash },
                ..Default::default()
            };
            let snapshot = shucked_command::host::capture_current(&context, 0);
            let target = TargetInventory::capture(label, &context, &snapshot);
            let json = target.to_json()?;
            if let Some(path) = output {
                // Export may expose filesystem paths; create private files and never overwrite.
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
                let mut file = options.open(&path).with_context(|| format!("Create {}", path.display()))?;
                writeln!(file, "{json}")?;
            } else { println!("{json}"); }
        }
        TargetCommand::Inspect { inventory } => {
            let target = read_inventory(&inventory)?;
            println!("{}", serde_json::to_string_pretty(&target)?);
        }
        TargetCommand::Compare { targets, script } => {
            let targets = targets.iter().map(|path| read_inventory(path)).collect::<Result<Vec<_>>>()?;
            let source = std::fs::read_to_string(&script)?;
            let facts = if script.extension().is_some_and(|ext| ext == "fish") {
                shucked_semantic::analyze_fish(&source).commands
            } else {
                let dialect = shucked_linter::ShellDialect::infer(&source, Some(&script));
                let profile = dialect.shell_profile();
                let parsed = shucked_parser::parser::Parser::with_profile(&source, profile.clone()).without_alias_expansion().parse();
                let index = shucked_indexer::Indexer::new(&source, &parsed);
                let semantic = shucked_semantic::SemanticModel::build_with_options(&parsed.file, &source, &index,
                    shucked_semantic::SemanticBuildOptions { source_path: Some(&script), shell_profile: Some(profile), ..Default::default() });
                semantic.command_site_facts()
            };
            let sites: Vec<_> = facts.iter().map(|fact| CommandSite {
                name: fact.name().map(str::to_owned), arguments: fact.effective_words.iter().skip(1).filter_map(|word| word.text.clone()).collect(),
                alias_eligible: false,
                environment_uncertain: fact.environment_uncertain.is_some(),
                functions: if fact.visible_function.is_some() { fact.name().into_iter().map(str::to_owned).collect() } else { Default::default() },
                guarded: if fact.guarded_available { fact.name().into_iter().map(str::to_owned).collect() } else { Default::default() },
                lookup: match fact.namespace { shucked_semantic::CommandNamespace::Builtin => shucked_command::LookupMode::BuiltinOnly, shucked_semantic::CommandNamespace::External => shucked_command::LookupMode::ExternalOnly, shucked_semantic::CommandNamespace::ExternalOrBuiltin => shucked_command::LookupMode::Command, _ => shucked_command::LookupMode::Normal },
                ..Default::default()
            }).collect();
            let comparison = shucked_command::compare_targets(&targets, &sites);
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                "script": script, "comparison": comparison,
                "locations": facts.iter().map(|fact| serde_json::json!({"line": fact.name_span().start.line(), "column": fact.name_span().start.column()})).collect::<Vec<_>>()
            }))?);
        }
    }
    Ok(ExitStatus::Success)
}

fn read_inventory(path: &Path) -> Result<TargetInventory> {
    let mut source = String::new();
    std::fs::File::open(path)?.take((shucked_command::MAX_INVENTORY_BYTES + 1) as u64).read_to_string(&mut source)?;
    TargetInventory::from_json(&source).with_context(|| format!("Read target {}", path.display()))
}
