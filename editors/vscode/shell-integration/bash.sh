# Sourced once in an explicitly attached interactive Bash session.
__shucked_generation=0
__shucked_capture() {
    local __shucked_status=$? __shucked_name
    __shucked_generation=$((__shucked_generation + 1))
    {
        builtin printf 'cwd\0%s\0' "$PWD"
        builtin printf 'searchpath\0%s\0' "$PATH"
        while IFS= read -r __shucked_name; do builtin printf 'alias\0%s\0' "$__shucked_name"; done < <(builtin alias -p)
        while IFS= read -r __shucked_name; do builtin printf 'function\0%s\0' "$__shucked_name"; done < <(builtin compgen -A function)
        builtin printf 'option\0expand_aliases=%s\0' "$(builtin shopt -q expand_aliases && builtin printf 1 || builtin printf 0)"
        case :${HISTCONTROL-}: in *:ignorespace:*|*:ignoreboth:*) builtin printf 'ignore\0leading-space\0';; esac
        if [[ -n ${HISTIGNORE-} ]]; then builtin printf 'private\0%s\0' 1; fi
        if [[ ! -o history ]]; then builtin printf 'private\0%s\0' 1; fi
    } | ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_CAPTURE" "$__shucked_generation" "$$" bash >/dev/null 2>&1
    return "$__shucked_status"
}
if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == 'declare -a'* ]]; then
    PROMPT_COMMAND+=(__shucked_capture)
else
    PROMPT_COMMAND="${PROMPT_COMMAND:+$PROMPT_COMMAND; }__shucked_capture"
fi
