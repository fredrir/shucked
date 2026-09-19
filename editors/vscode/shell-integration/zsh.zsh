# Sourced once in an explicitly attached interactive Zsh session.
__shucked_generation=0
__shucked_capture() {
    local __shucked_status=$? __shucked_name __shucked_part
    (( __shucked_generation += 1 ))
    {
        builtin printf 'cwd\0%s\0' "$PWD"
        for __shucked_part in "${path[@]}"; do builtin printf 'path\0%s\0' "$__shucked_part"; done
        for __shucked_name in "${(@k)aliases}"; do builtin printf 'alias\0%s=%s\0' "$__shucked_name" "${aliases[$__shucked_name]}"; done
        for __shucked_name in "${(@k)functions}"; do builtin printf 'function\0%s\0' "$__shucked_name"; done
        builtin printf 'option\0aliases=%s\0' "${options[aliases]}"
        [[ -o histignorespace ]] && builtin printf 'ignore\0leading-space\0'
        (( HISTSIZE <= 0 )) && builtin printf 'private\0%s\0' 1
        (( ${+functions[zshaddhistory]} || ${#zshaddhistory_functions} )) && builtin printf 'private\0%s\0' 1
        [[ -n ${HISTORY_IGNORE-} ]] && builtin printf 'private\0%s\0' 1
    } | ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_CAPTURE" "$__shucked_generation" "$$" zsh >/dev/null 2>&1
    return $__shucked_status
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __shucked_capture
