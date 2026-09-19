# Versioned option grammars

These records describe option names and argument arity, including hidden aliases and negations. They contain interface facts and source references, not upstream implementation code or help text. The resolver requires the installed executable's exact version before using a record to reject a flag.

| Tool | Covered versions | Boundary |
| --- | --- | --- |
| eza | 0.23.0–0.23.5 | Option names; values and cross-option conflicts remain unchecked |
| ripgrep | 14.1.1, 15.1.0, 15.2.0 | Includes hidden negated options and aliases |
| fd | 10.3.0 | Execution tails after `--exec`/`--exec-batch` remain Unknown |
| bat | 0.25.0 | Initial options only; positional/subcommand contexts remain Unknown |
| GNU ls | 9.7 | Abbreviated long options remain Unknown |
| curl | 8.7.1, 8.12.1, 8.14.1, 8.15.0 | Boolean negations and expanded values; configuration-loading scopes remain Unknown |
| Pacman | 7.0.0, 7.1.0 | Union of operation option names; operation-specific validity remains unchecked |

Each JSON file links to the corresponding version's option registration. Updating coverage requires reviewing parser changes and testing aliases, argument consumption, end-of-options, and delegated arguments. A similar help listing or matching major version is insufficient. Other versions and unidentified BSD `ls` builds remain Unknown.
