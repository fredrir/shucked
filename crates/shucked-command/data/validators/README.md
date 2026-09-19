# Versioned option grammars

These records describe option names and argument arity, including hidden aliases and negations. They contain interface facts and source references, not upstream implementation code or help text. The resolver requires the installed executable's exact version before using a record to reject a flag.

| Tool | Covered versions | Boundary |
| --- | --- | --- |
| eza | 0.23.0–0.23.5 | Option names; values and cross-option conflicts remain unchecked |
| ripgrep | 14.1.1, 15.1.0, 15.2.0 | Includes hidden negated options and aliases |
| fd | 10.3.0 | Execution tails after `--exec`/`--exec-batch` remain Unknown |
| bat | 0.25.0 | Initial options only; positional/subcommand contexts remain Unknown |
| GNU ls | 9.7 | Abbreviated long options remain Unknown |
| Apple BSD ls | file_cmds 457.140.3, 475, 479 | Embedded component identity; options before the first pathname; unidentified/newer builds remain Unknown |
| OpenSSH | 9.8p1, 9.9p2, 10.0p1–10.5p1 | Fixed `-V` query; options before the destination; remote command arguments remain Unknown |
| Docker | 27.5.1, 28.0.0 | Root options/commands, including hidden legacy commands; installed plugin names scanned without executing plugins; child arguments remain Unknown |
| kubectl | 1.32.0, 1.33.0, 1.34.0 | Client-only JSON version metadata and complete PATH plugin inventory; bare first subcommands only; KubeRC preferences remain Unknown |
| curl | 8.7.1, 8.12.1, 8.14.1, 8.15.0 | Boolean negations and expanded values; configuration-loading scopes remain Unknown |
| Pacman | 7.0.0, 7.1.0 | Union of operation option names; operation-specific validity remains unchecked |

Each JSON file links to the corresponding version's option registration. Updating coverage requires reviewing parser changes and testing aliases, argument consumption, end-of-options, and delegated arguments. A similar help listing or matching major version is insufficient. Other versions and unidentified BSD `ls` builds remain Unknown.

`shucked target capture --capabilities` records these grammars for offline comparison. Plain `target capture` performs filesystem inspection only. Docker/kubectl capability acquisition currently stays Unknown for attached terminal sessions because their configuration environment variables are not part of session metadata.
