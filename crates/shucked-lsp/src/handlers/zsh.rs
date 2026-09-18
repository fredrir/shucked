//! Zsh language intelligence for the Shucked language server.
//!
//! Provides documentation, signatures, and metadata for Zsh builtins,
//! special runtime parameters, and shell options.
//!
//! Clean-room reimplementation based strictly on standard Zsh manuals
//! and shell specifications.

/// Documentation for a Zsh shell builtin.
pub struct ZshBuiltinDoc {
    /// Command signature / syntax outline.
    pub signature: &'static str,
    /// Detailed Markdown documentation.
    pub markdown: &'static str,
}

/// Retrieve signature and Markdown documentation for a known Zsh builtin.
pub fn builtin_doc(name: &str) -> Option<ZshBuiltinDoc> {
    match name {
        "zstyle" => Some(ZshBuiltinDoc {
            signature: "zstyle [ -e | - | -- ] pattern style string ...",
            markdown: "### `zstyle` (Zsh Builtin)\n\
```zsh\n\
zstyle [ -e | - | -- ] pattern style string ...\n\
zstyle -d [ pattern [ style ... ] ]\n\
zstyle -g name [ pattern [ style ] ]\n\
zstyle -s pattern style name [ sep ]\n\
zstyle -b pattern style name\n\
zstyle -a pattern style name\n\
zstyle -t pattern style [ string ... ]\n\
zstyle -T pattern style [ string ... ]\n\
zstyle -m pattern style regex\n\
```\n\n\
Configures and queries user styles for Zsh subsystems, such as the programmable completion system (`compinit`), widgets, and VCS integration (`vcs_info`).\n\n\
Styles are matched against hierarchical context patterns of the form:\n\
`:completion:<function>:<completer>:<command>:<argument>:<tag>`\n\n\
Common forms:\n\
- `zstyle pattern style string ...`: Sets `style` to the given string values for matching contexts.\n\
- `zstyle -d [ pattern [ style ... ] ]`: Deletes matching style definitions.\n\
- `zstyle -b pattern style name`: Evaluates a boolean style and sets parameter `name` to `yes` or `no`.\n\
- `zstyle -s pattern style name [ sep ]`: Retrieves a scalar style into `name` (elements separated by `sep`).\n\
- `zstyle -a pattern style name`: Retrieves an array style into `name`.\n\
- `zstyle -t pattern style [ strings ... ]`: Tests if a boolean style evaluates to true.\n\
- `zstyle -e pattern style string`: Sets style dynamically by evaluating `string` on each lookup.",
        }),
        "autoload" => Some(ZshBuiltinDoc {
            signature: "autoload [ {+|-}UXktz ] [ -w ] [ name ... ]",
            markdown: "### `autoload` (Zsh Builtin)\n\
```zsh\n\
autoload [ {+|-}UXktz ] [ -w ] [ name ... ]\n\
```\n\n\
Marks functions to be automatically loaded from directory paths in `$fpath` upon first call.\n\n\
Flags:\n\
- `-U`: Suppress alias expansion when parsing and loading the function body.\n\
- `-z`: Use native Zsh execution semantics for the loaded function.\n\
- `-k`: Use Korn shell (ksh) compatibility semantics for the loaded function.\n\
- `-X`: Immediately load the function definition from `$fpath` rather than waiting for invocation.\n\
- `-w`: Treat operands as precompiled Zsh word code files (`.zwc`).",
        }),
        "compdef" => Some(ZshBuiltinDoc {
            signature: "compdef [ -an ] function name ...",
            markdown: "### `compdef` (Zsh Completion)\n\
```zsh\n\
compdef [ -an ] function name ...\n\
compdef -d name ...\n\
compdef -k [ -an ] function style key-sequence ...\n\
```\n\n\
Registers completion functions with the Zsh completion engine (`compinit`), associating `function` with commands or key sequences.\n\n\
Flags:\n\
- `-a`: Automatically autoload `function` before the first completion attempt.\n\
- `-n`: Do not overwrite existing completion definitions for `name`.\n\
- `-d`: Remove existing completion definitions for `name`.\n\
- `-k`: Bind completion function to editor key sequences using `style`.",
        }),
        "bindkey" => Some(ZshBuiltinDoc {
            signature: "bindkey [ options ] [ in-string [ out-string ] ]",
            markdown: "### `bindkey` (Zsh Builtin)\n\
```zsh\n\
bindkey [ options ] [ in-string [ out-string ] ]\n\
```\n\n\
Manipulates keymaps and key bindings for the Zsh Line Editor (ZLE).\n\n\
Key options:\n\
- `-e`: Select the Emacs keymap as active.\n\
- `-v`: Select the Vi command keymap as active.\n\
- `-a`: Select the Vi alternate (command mode) keymap.\n\
- `-l`: List all registered keymap names.\n\
- `-M <map>`: Operate on the specified keymap instead of the active one.\n\
- `-s <in> <out>`: Bind key sequence `<in>` to output macro `<out>`.\n\
- `-r <in>`: Unbind key sequence `<in>`.\n\
- `bindkey <key> <widget>`: Bind key sequence to an editor widget.",
        }),
        "vared" => Some(ZshBuiltinDoc {
            signature: "vared [ -Aachept ] [ -p prompt ] [ -r rprompt ] [ -m msg ] [ -h ] variable",
            markdown: "### `vared` (Zsh Builtin)\n\
```zsh\n\
vared [ -Aachept ] [ -p prompt ] [ -r rprompt ] [ -m msg ] [ -h ] variable\n\
```\n\n\
Interactively edits the value of `variable` using the Zsh Line Editor (ZLE).\n\n\
Options:\n\
- `-p <prompt>`: Display `<prompt>` on the left side during editing.\n\
- `-r <rprompt>`: Display `<rprompt>` on the right margin.\n\
- `-h`: Enable history navigation while editing.\n\
- `-c`: Create the parameter if it is not already set.\n\
- `-a`: Edit an array parameter.\n\
- `-A`: Edit an associative array parameter.",
        }),
        "typeset" => Some(ZshBuiltinDoc {
            signature: "typeset [ {+|-}AHUFLRUXZafghilrtux ] [ {+|-}E ] [ -p ] [ name[=value] ... ]",
            markdown: "### `typeset` (Zsh Builtin)\n\
```zsh\n\
typeset [ {+|-}AHUFLRUXZafghilrtux ] [ {+|-}E ] [ -p ] [ name[=value] ... ]\n\
```\n\n\
Sets attributes and values for shell parameters, or displays existing definitions.\n\n\
Common flags:\n\
- `-a`: Declare parameter as an indexed array.\n\
- `-A`: Declare parameter as an associative array (hash table).\n\
- `-i [n]`: Declare parameter as an integer with optional arithmetic base `n`.\n\
- `-F [n]`: Declare parameter as a fixed-point float with optional precision `n`.\n\
- `-E [n]`: Declare parameter as a floating-point number in scientific notation.\n\
- `-r`: Mark parameter as read-only.\n\
- `-x`: Export parameter to the environment for child processes.\n\
- `-u`: Convert parameter value to uppercase upon assignment.\n\
- `-l`: Convert parameter value to lowercase upon assignment.\n\
- `-g`: Create global parameter from inside a local function scope.\n\
- `-U`: Maintain unique values only (automatically deduplicate array elements).\n\
- `-p`: Output parameter definitions in command format.",
        }),
        "print" => Some(ZshBuiltinDoc {
            signature: "print [ -abcDeEfimnNprRsTvz ] [ -u fd ] [ -f format ] [ -C cols ] [ -o | -O ] [ -S ] [ -P ] [ arg ... ]",
            markdown: "### `print` (Zsh Builtin)\n\
```zsh\n\
print [ -abcDeEfimnNprRsTvz ] [ -u fd ] [ -f format ] [ -C cols ] [ -o | -O ] [ -S ] [ -P ] [ arg ... ]\n\
```\n\n\
Outputs arguments to standard output or a designated file descriptor with specialized formatting, sorting, and prompt expansion.\n\n\
Common flags:\n\
- `-r`: Raw output; do not interpret backslash escape sequences.\n\
- `-P`: Perform prompt expansion (`%~`, `%n`, `%F{color}`) before printing.\n\
- `-l`: Print each argument on a separate newline.\n\
- `-n`: Suppress trailing newline.\n\
- `-u <fd>`: Direct output to file descriptor `<fd>`.\n\
- `-z`: Push arguments onto the ZLE input buffer stack.\n\
- `-v <var>`: Store formatted output in parameter `<var>` instead of printing.\n\
- `-o`: Sort arguments in ascending order before printing.\n\
- `-O`: Sort arguments in descending order before printing.\n\
- `-c`: Output arguments in multiple columns.",
        }),
        "setopt" => Some(ZshBuiltinDoc {
            signature: "setopt [ {+|-}options ... ] [ option ... ]",
            markdown: "### `setopt` (Zsh Builtin)\n\
```zsh\n\
setopt [ {+|-}options ... ] [ option ... ]\n\
```\n\n\
Enables shell options or lists active configuration options.\n\n\
Usage:\n\
- `setopt OPTION`: Enables the specified option.\n\
- `setopt no_OPTION` or `setopt +o OPTION`: Disables the option.\n\
- Without arguments, lists all options currently set that differ from defaults.\n\
- Option names are case-insensitive and underscores are ignored (e.g., `NULL_GLOB`, `nullglob`, `Null_Glob`).",
        }),
        "unsetopt" => Some(ZshBuiltinDoc {
            signature: "unsetopt [ option ... ]",
            markdown: "### `unsetopt` (Zsh Builtin)\n\
```zsh\n\
unsetopt [ option ... ]\n\
```\n\n\
Disables one or more shell options.\n\n\
Usage:\n\
- `unsetopt OPTION`: Disables the specified option.\n\
- Option names are case-insensitive and ignore underscores (e.g., `unsetopt extended_glob`).\n\
- Equivalent to `setopt no_<option>`.",
        }),
        "add-zsh-hook" => Some(ZshBuiltinDoc {
            signature: "add-zsh-hook [ -dD ] [ -U ] hook function",
            markdown: "### `add-zsh-hook` (Zsh Function)\n\
```zsh\n\
add-zsh-hook [ -dD ] [ -U ] hook function\n\
```\n\n\
Registers or unregisters a shell function to execute when a specific Zsh lifecycle hook fires.\n\n\
Supported hooks:\n\
- `chpwd`: Invoked whenever the current working directory changes.\n\
- `precmd`: Invoked before reading a new command prompt.\n\
- `preexec`: Invoked after a command line is read but before execution.\n\
- `periodic`: Invoked periodically based on `$PERIOD`.\n\
- `zshaddhistory`: Invoked before adding a command line to history.\n\
- `zshexit`: Invoked right before the shell process exits.\n\n\
Flags:\n\
- `-d`: Unregister `function` from `hook`.\n\
- `-D`: Unregister all functions matching pattern from `hook`.\n\
- `-U`: Autoload function with alias expansion suppressed.",
        }),
        "zle" => Some(ZshBuiltinDoc {
            signature: "zle [ -N | -C | -D | -A | -la | -f | -U | -K ] [ args ... ]",
            markdown: "### `zle` (Zsh Builtin)\n\
```zsh\n\
zle -N widget [ function ]\n\
zle -C widget completion-widget function\n\
zle -D widget ...\n\
zle -A old-widget new-widget\n\
zle -la [ -m ]\n\
zle widget-name [ args ... ]\n\
```\n\n\
Manipulates and creates Zsh Line Editor (ZLE) user-defined widgets, keymaps, and buffer operations.",
        }),
        "zmodload" => Some(ZshBuiltinDoc {
            signature: "zmodload [ -d | -u | -a | -e | -b | -c | -p ] [ name ... ]",
            markdown: "### `zmodload` (Zsh Builtin)\n\
```zsh\n\
zmodload [ -d | -u | -a | -e | -b | -c | -p ] [ name ... ]\n\
```\n\n\
Loads or unloads dynamically-loadable binary modules (e.g., `zsh/stat`, `zsh/zpty`, `zsh/pcre`, `zsh/datetime`, `zsh/net/tcp`).",
        }),
        "emulate" => Some(ZshBuiltinDoc {
            signature: "emulate [ -LR ] [ {zsh|sh|ksh|csh} [ -c cmd ] ]",
            markdown: "### `emulate` (Zsh Builtin)\n\
```zsh\n\
emulate [ -LR ] [ {zsh|sh|ksh|csh} [ -c cmd ] ]\n\
```\n\n\
Switches option defaults and parsing rules to emulate target shell behaviors (`zsh`, `sh`, `ksh`, or `csh`).\n\n\
Flags:\n\
- `-L`: Localize option changes to the enclosing function scope (enables `LOCAL_OPTIONS` and `LOCAL_TRAPS`).\n\
- `-R`: Reset options back to absolute emulation defaults rather than preserving active options.\n\
- `-c cmd`: Execute `cmd` under emulation mode and restore prior options afterwards.",
        }),
        "compinit" => Some(ZshBuiltinDoc {
            signature: "compinit [ -d dumpfile ] [ -D ] [ -u ] [ -C ]",
            markdown: "### `compinit` (Zsh Completion Initializer)\n\
```zsh\n\
compinit [ -d dumpfile ] [ -D ] [ -u ] [ -C ]\n\
```\n\n\
Initializes the Zsh completion system, scanning `$fpath` directories for `#compdef` and `#autoload` tags and setting up completion widgets.",
        }),
        "compadd" => Some(ZshBuiltinDoc {
            signature: "compadd [ options ] [ candidate ... ]",
            markdown: "### `compadd` (Zsh Completion Builtin)\n\
```zsh\n\
compadd [ options ] [ candidate ... ]\n\
```\n\n\
Core completion primitive that passes candidate completion words and display descriptions to the ZLE completion engine.",
        }),
        "whence" | "which" | "where" => Some(ZshBuiltinDoc {
            signature: "whence [ -acvpcsfam ] name ...",
            markdown: "### `whence` / `which` / `where` (Zsh Builtin)\n\
```zsh\n\
whence [ -acvpcsfam ] name ...\n\
which [ -acvpcsfam ] name ...\n\
where [ -acvpcsfam ] name ...\n\
```\n\n\
Inspects command resolution and classifies how `name` is interpreted (alias, builtin, reserved word, function, or external executable).\n\n\
Flags:\n\
- `-v`: Verbose description of command type and resolution.\n\
- `-c`: Output in csh-style format.\n\
- `-a`: Find all occurrences along the path, not just the first.",
        }),
        "unfunction" => Some(ZshBuiltinDoc {
            signature: "unfunction [ -m ] name ...",
            markdown: "### `unfunction` (Zsh Builtin)\n\
```zsh\n\
unfunction [ -m ] name ...\n\
```\n\n\
Removes shell function definitions from the running environment.\n\n\
- `-m`: Treat arguments as patterns and remove all matching functions.",
        }),
        "float" => Some(ZshBuiltinDoc {
            signature: "float [ {+|-}EFHUXZghlprtux ] [ name[=value] ... ]",
            markdown: "### `float` (Zsh Builtin)\n\
```zsh\n\
float [ options ] [ name[=value] ... ]\n\
```\n\n\
Equivalent to `typeset -E`; declares parameters with floating-point numeric attributes.",
        }),
        "integer" => Some(ZshBuiltinDoc {
            signature: "integer [ {+|-}HUXZghlprtux ] [ name[=value] ... ]",
            markdown: "### `integer` (Zsh Builtin)\n\
```zsh\n\
integer [ options ] [ name[=value] ... ]\n\
```\n\n\
Equivalent to `typeset -i`; declares parameters with integer numeric attributes.",
        }),
        "local" => Some(ZshBuiltinDoc {
            signature: "local [ options ] [ name[=value] ... ]",
            markdown: "### `local` (Zsh Builtin)\n\
```zsh\n\
local [ options ] [ name[=value] ... ]\n\
```\n\n\
Declares variables scoped locally to the current function or block.",
        }),
        "echoti" => Some(ZshBuiltinDoc {
            signature: "echoti cap [ arg ... ]",
            markdown: "### `echoti` (Zsh Builtin)\n\
```zsh\n\
echoti cap [ arg ... ]\n\
```\n\n\
Outputs the terminfo capability string for `cap` with optional parameters instantiated.",
        }),
        "echotc" => Some(ZshBuiltinDoc {
            signature: "echotc cap [ arg ... ]",
            markdown: "### `echotc` (Zsh Builtin)\n\
```zsh\n\
echotc cap [ arg ... ]\n\
```\n\n\
Outputs the termcap capability string for `cap` with optional parameters instantiated.",
        }),
        _ => None,
    }
}

/// Documentation for a Zsh special parameter.
pub struct ZshParameterDoc {
    /// Parameter data type description (e.g. `Array of integers`, `Scalar`).
    pub param_type: &'static str,
    /// Detailed Markdown documentation.
    pub markdown: &'static str,
}

/// Retrieve type and Markdown documentation for a special Zsh runtime parameter.
pub fn special_parameter_doc(name: &str) -> Option<ZshParameterDoc> {
    match name {
        "pipestatus" => Some(ZshParameterDoc {
            param_type: "Array of integers",
            markdown: "Holds the exit status values for each individual command executed in the most recently completed pipeline.\n\n\
```zsh\n\
cat /missing | sort | uniq\n\
echo $pipestatus[1]  # exit status of 'cat'\n\
echo $pipestatus[2]  # exit status of 'sort'\n\
```",
        }),
        "match" => Some(ZshParameterDoc {
            param_type: "Array of strings",
            markdown: "Contains substrings captured by parenthesized groups during pattern matching and regular expression evaluation when the `(#b)` globbing flag is active or via the `=~` operator.",
        }),
        "mbegin" => Some(ZshParameterDoc {
            param_type: "Array of integers (1-based indices)",
            markdown: "Contains the 1-based start offsets of matched substrings captured by parenthesized groups during `(#b)` pattern matching.",
        }),
        "mend" => Some(ZshParameterDoc {
            param_type: "Array of integers (1-based indices)",
            markdown: "Contains the 1-based end offsets of matched substrings captured by parenthesized groups during `(#b)` pattern matching.",
        }),
        "path" => Some(ZshParameterDoc {
            param_type: "Array of directory paths (tied to $PATH)",
            markdown: "Colonic tied array representing directory paths searched for executable programs. Modifying `$path` automatically updates `$PATH`, and vice-versa.\n\n\
```zsh\n\
path=('/usr/local/bin' $path)\n\
path+=(\"$HOME/.local/bin\")\n\
```",
        }),
        "fpath" => Some(ZshParameterDoc {
            param_type: "Array of directory paths (tied to $FPATH)",
            markdown: "List of directory paths searched by the shell to discover and autoload function definitions (used by `autoload` and `compinit`).\n\n\
```zsh\n\
fpath=(\"$HOME/.zsh/completions\" $fpath)\n\
```",
        }),
        "cdpath" => Some(ZshParameterDoc {
            param_type: "Array of directory paths (tied to $CDPATH)",
            markdown: "Search path used by the `cd` command when changing to a directory specified without a leading slash.",
        }),
        "ZSH_VERSION" => Some(ZshParameterDoc {
            param_type: "String",
            markdown: "The version string of the currently running Zsh shell (e.g. `\"5.9\"`).",
        }),
        "status" => Some(ZshParameterDoc {
            param_type: "Integer",
            markdown: "The exit status of the last executed command. Equivalent to `$?`.",
        }),
        "prompt" | "PROMPT" => Some(ZshParameterDoc {
            param_type: "String",
            markdown: "The primary prompt string displayed before reading interactive shell commands. Equivalent to `$PS1`.\n\n\
Supports prompt expansion escapes such as `%~` (working directory), `%n` (username), `%m` (hostname), and `%#` (`#` for root, `%` for ordinary user).",
        }),
        "PROMPT2" | "RPROMPT" | "RPS1" => Some(ZshParameterDoc {
            param_type: "String",
            markdown: "Secondary or right-hand prompt string displayed during command line input.",
        }),
        "signals" => Some(ZshParameterDoc {
            param_type: "Array of strings",
            markdown: "Names of signals supported by the current system.",
        }),
        "terminfo" => Some(ZshParameterDoc {
            param_type: "Associative array",
            markdown: "Associative array mapping terminfo capability names to their active terminal values.",
        }),
        "termcap" => Some(ZshParameterDoc {
            param_type: "Associative array",
            markdown: "Associative array mapping termcap capability names to their active terminal values.",
        }),
        "dirstack" => Some(ZshParameterDoc {
            param_type: "Array of directory paths",
            markdown: "Array representing the current directory stack maintained by `pushd` and `popd`.",
        }),
        "aliases" => Some(ZshParameterDoc {
            param_type: "Associative array",
            markdown: "Associative array mapping defined alias names to their expansion text.",
        }),
        "functions" => Some(ZshParameterDoc {
            param_type: "Associative array",
            markdown: "Associative array mapping defined function names to their body text.",
        }),
        "commands" => Some(ZshParameterDoc {
            param_type: "Associative array",
            markdown: "Associative array mapping external command names to their resolved filesystem paths in `$path`.",
        }),
        "parameters" => Some(ZshParameterDoc {
            param_type: "Associative array",
            markdown: "Associative array mapping parameter names to their type descriptions.",
        }),
        "options" => Some(ZshParameterDoc {
            param_type: "Associative array",
            markdown: "Associative array mapping shell option names to their state (`on` or `off`).",
        }),
        _ => None,
    }
}

/// Normalize an option string for matching: lowercase, strip `_` and `-`.
/// Returns `(normalized_key, is_inverted)`.
pub fn normalize_option(s: &str) -> (String, bool) {
    let mut normalized = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '_' | '-') {
            continue;
        }
        normalized.push(ch.to_ascii_lowercase());
    }

    if let Some(rest) = normalized.strip_prefix("no") {
        (rest.to_owned(), true)
    } else {
        (normalized, false)
    }
}

/// Information about a Zsh shell option.
pub struct ZshOptionDoc {
    /// Canonical name in UPPER_SNAKE_CASE.
    pub canonical_name: &'static str,
    /// Detailed description.
    pub description: &'static str,
    /// Default state in native Zsh.
    pub default_on: bool,
}

/// Retrieve documentation for a Zsh option by name (case-insensitive, ignoring underscores).
pub fn option_doc(name: &str) -> Option<(ZshOptionDoc, bool)> {
    let (normalized, inverted) = normalize_option(name);
    let doc = match normalized.as_str() {
        "nullglob" => ZshOptionDoc {
            canonical_name: "NULL_GLOB",
            description: "If a pattern for filename generation has no matches, delete the pattern from the argument list entirely instead of reporting an error.",
            default_on: false,
        },
        "extendedglob" => ZshOptionDoc {
            canonical_name: "EXTENDED_GLOB",
            description: "Treats `#`, `~`, and `^` as pattern matching operators for filename generation and string pattern matching.",
            default_on: false,
        },
        "errexit" => ZshOptionDoc {
            canonical_name: "ERR_EXIT",
            description: "If a command has a non-zero exit status, execute the ZERR trap if set and exit the shell immediately. Equivalent to `set -e`.",
            default_on: false,
        },
        "promptsubst" => ZshOptionDoc {
            canonical_name: "PROMPT_SUBST",
            description: "Enables parameter expansion, command substitution, and arithmetic evaluation inside prompt strings (`$PROMPT`, `$PS1`, etc.).",
            default_on: false,
        },
        "ksharrays" => ZshOptionDoc {
            canonical_name: "KSH_ARRAYS",
            description: "Emulates Korn shell array semantics: arrays are 0-indexed instead of 1-indexed, and `$arr` refers to element 0 rather than all elements.",
            default_on: false,
        },
        "nomatch" => ZshOptionDoc {
            canonical_name: "NO_MATCH",
            description: "Reports an error if a pattern for filename generation has no matches. (Enabled by default; disable to leave unmatched patterns unmodified).",
            default_on: true,
        },
        "globdots" => ZshOptionDoc {
            canonical_name: "GLOB_DOTS",
            description: "Do not require a leading `.` in a filename to be matched explicitly in glob patterns.",
            default_on: false,
        },
        "shwordsplit" => ZshOptionDoc {
            canonical_name: "SH_WORD_SPLIT",
            description: "Performs field splitting on unquoted parameter expansions, matching POSIX and Bourne shell behavior rather than preserving variable values intact.",
            default_on: false,
        },
        "rcexpandparam" => ZshOptionDoc {
            canonical_name: "RC_EXPAND_PARAM",
            description: "Expands array parameters using `rc`-style brace syntax: `foo${arr}bar` expands by prefixing and suffixing each element of `arr`.",
            default_on: false,
        },
        "kshglob" => ZshOptionDoc {
            canonical_name: "KSH_GLOB",
            description: "Enables ksh-style extended globbing patterns: `@(...)`, `*(...)`, `+(...)`, `?(...)`, `!(...)`.",
            default_on: false,
        },
        "shglob" => ZshOptionDoc {
            canonical_name: "SH_GLOB",
            description: "Disables special Zsh globbing operators that conflict with POSIX shell syntax.",
            default_on: false,
        },
        "bareglobqual" => ZshOptionDoc {
            canonical_name: "BARE_GLOB_QUAL",
            description: "Interprets bare trailing qualifiers `*(.)`, `*(/)` at the end of glob patterns without requiring explicit `(#q)` prefixes.",
            default_on: true,
        },
        "equals" => ZshOptionDoc {
            canonical_name: "EQUALS",
            description: "Performs command path expansion on arguments beginning with `=` (e.g. `=ls` expands to `/bin/ls`).",
            default_on: true,
        },
        "magicequalsubst" => ZshOptionDoc {
            canonical_name: "MAGIC_EQUAL_SUBST",
            description: "Performs filename expansion after any `=` sign in command arguments, as if the argument were an assignment.",
            default_on: false,
        },
        "globassign" => ZshOptionDoc {
            canonical_name: "GLOB_ASSIGN",
            description: "Treats the right-hand side of scalar variable assignments as eligible for glob expansion.",
            default_on: false,
        },
        "ignorebraces" => ZshOptionDoc {
            canonical_name: "IGNORE_BRACES",
            description: "Treats brace characters `{` and `}` literally rather than performing brace expansion.",
            default_on: false,
        },
        "ignoreclosebraces" => ZshOptionDoc {
            canonical_name: "IGNORE_CLOSE_BRACES",
            description: "Treats unmatched closing braces `}` literally instead of reporting a syntax error.",
            default_on: false,
        },
        "braceccl" => ZshOptionDoc {
            canonical_name: "BRACE_CCL",
            description: "Expands character sets in braces, such as `{a-z}` expanding to all lowercase letters.",
            default_on: false,
        },
        "shortloops" => ZshOptionDoc {
            canonical_name: "SHORT_LOOPS",
            description: "Enables short loop forms such as `for x (a b c) print $x`.",
            default_on: true,
        },
        "shortrepeat" => ZshOptionDoc {
            canonical_name: "SHORT_REPEAT",
            description: "Enables short `repeat` loops such as `repeat 3 echo hi`.",
            default_on: true,
        },
        "rcquotes" => ZshOptionDoc {
            canonical_name: "RC_QUOTES",
            description: "Decodes paired single quotes `''` inside single-quoted strings as a literal single quote.",
            default_on: false,
        },
        "interactivecomments" => ZshOptionDoc {
            canonical_name: "INTERACTIVE_COMMENTS",
            description: "Recognizes `#` as starting a comment even in interactive shell sessions.",
            default_on: true,
        },
        "cbases" => ZshOptionDoc {
            canonical_name: "C_BASES",
            description: "Outputs hexadecimal and octal numbers with C-style prefixes (`0x`, `0`) rather than `base#value`.",
            default_on: false,
        },
        "octalzeroes" => ZshOptionDoc {
            canonical_name: "OCTAL_ZEROES",
            description: "Interprets integer literals with a leading zero as octal constants.",
            default_on: false,
        },
        "cshnullglob" => ZshOptionDoc {
            canonical_name: "CSH_NULL_GLOB",
            description: "In commands with multiple patterns, deletes unmatched patterns if at least one matched; reports an error only if no patterns matched.",
            default_on: false,
        },
        "autocd" => ZshOptionDoc {
            canonical_name: "AUTO_CD",
            description: "If a command name cannot be executed and is the name of a directory, change directory to it automatically.",
            default_on: false,
        },
        "correct" => ZshOptionDoc {
            canonical_name: "CORRECT",
            description: "Attempts to spell-check and autocorrect command names entered interactively.",
            default_on: false,
        },
        "correctall" => ZshOptionDoc {
            canonical_name: "CORRECT_ALL",
            description: "Attempts to spell-check and autocorrect all arguments in entered command lines.",
            default_on: false,
        },
        "histignoredups" => ZshOptionDoc {
            canonical_name: "HIST_IGNORE_DUPS",
            description: "Does not enter a command line into the history list if it duplicates the previous entry.",
            default_on: false,
        },
        "histignorealldups" => ZshOptionDoc {
            canonical_name: "HIST_IGNORE_ALL_DUPS",
            description: "Removes older duplicate entries from the history list when a duplicate command line is executed.",
            default_on: false,
        },
        "histignorespace" => ZshOptionDoc {
            canonical_name: "HIST_IGNORE_SPACE",
            description: "Omits commands starting with a leading space character from the history list.",
            default_on: false,
        },
        "sharehistory" => ZshOptionDoc {
            canonical_name: "SHARE_HISTORY",
            description: "Shares shell command history across concurrent running Zsh sessions in real time.",
            default_on: false,
        },
        "incappendhistory" => ZshOptionDoc {
            canonical_name: "INC_APPEND_HISTORY",
            description: "Appends commands to the history file immediately upon execution rather than waiting for shell exit.",
            default_on: false,
        },
        "extendedhistory" => ZshOptionDoc {
            canonical_name: "EXTENDED_HISTORY",
            description: "Records timestamps and execution duration metadata alongside commands in the history file.",
            default_on: false,
        },
        "pipefail" => ZshOptionDoc {
            canonical_name: "PIPE_FAIL",
            description: "Returns the exit status of the rightmost failed command in a pipeline, or 0 if all succeeded.",
            default_on: false,
        },
        "localoptions" => ZshOptionDoc {
            canonical_name: "LOCAL_OPTIONS",
            description: "Automatically restores shell options modified inside a function when the function returns.",
            default_on: false,
        },
        "localtraps" => ZshOptionDoc {
            canonical_name: "LOCAL_TRAPS",
            description: "Automatically restores signal traps modified inside a function when the function returns.",
            default_on: false,
        },
        "clobber" => ZshOptionDoc {
            canonical_name: "CLOBBER",
            description: "Allows `>` redirection to overwrite existing files without requiring `>!`.",
            default_on: true,
        },
        "noclobber" => ZshOptionDoc {
            canonical_name: "NO_CLOBBER",
            description: "Prevents `>` redirection from overwriting existing files without explicit `>!` override.",
            default_on: false,
        },
        "verbose" => ZshOptionDoc {
            canonical_name: "VERBOSE",
            description: "Prints shell input lines verbatim as they are read.",
            default_on: false,
        },
        "xtrace" => ZshOptionDoc {
            canonical_name: "XTRACE",
            description: "Prints commands and their arguments as they are executed (execution trace). Equivalent to `set -x`.",
            default_on: false,
        },
        "warncreateglobal" => ZshOptionDoc {
            canonical_name: "WARN_CREATE_GLOBAL",
            description: "Emits a warning when an unadorned assignment creates a global variable from inside a function.",
            default_on: false,
        },
        "functionargzero" => ZshOptionDoc {
            canonical_name: "FUNCTION_ARGZERO",
            description: "Sets `$0` to the name of the function inside function bodies.",
            default_on: true,
        },
        "markdirs" => ZshOptionDoc {
            canonical_name: "MARK_DIRS",
            description: "Appends a trailing slash `/` to all directory names generated by globbing.",
            default_on: false,
        },
        "numericglobsort" => ZshOptionDoc {
            canonical_name: "NUMERIC_GLOB_SORT",
            description: "Sorts filenames numerically rather than strictly lexicographically when numbers are present in names.",
            default_on: false,
        },
        "caseglob" => ZshOptionDoc {
            canonical_name: "CASE_GLOB",
            description: "Makes filename globbing sensitive to letter case.",
            default_on: true,
        },
        "banghist" => ZshOptionDoc {
            canonical_name: "BANG_HIST",
            description: "Enables C-shell style `!` history expansion.",
            default_on: true,
        },
        "autopushd" => ZshOptionDoc {
            canonical_name: "AUTO_PUSHD",
            description: "Makes `cd` automatically push the old directory onto the directory stack.",
            default_on: false,
        },
        "pushdignoredups" => ZshOptionDoc {
            canonical_name: "PUSHD_IGNORE_DUPS",
            description: "Prevents duplicate directories from being added to the directory stack.",
            default_on: false,
        },
        "pushdsilent" => ZshOptionDoc {
            canonical_name: "PUSH_D_SILENT",
            description: "Suppresses printing the directory stack after `pushd` or `popd`.",
            default_on: false,
        },
        _ => return None,
    };

    Some((doc, inverted))
}
