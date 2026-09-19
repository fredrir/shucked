# Sourced once in an explicitly attached interactive Zsh session.
__shucked_generation=0
__shucked_capture() {
    local __shucked_status=$? __shucked_name __shucked_part __shucked_history_policy __shucked_private=0
    (( __shucked_generation += 1 ))
    {
        builtin printf 'cwd\0%s\0' "$PWD"
        if [[ ${__shucked_live_signal-} == SIGUSR1 && ${functions[TRAPUSR1]-} == *__shucked_live_zsh_request* || ${__shucked_live_signal-} == SIGUSR2 && ${functions[TRAPUSR2]-} == *__shucked_live_zsh_request* ]]; then
            builtin printf 'live-signal\0%s\0' "$__shucked_live_signal"
        fi
        for __shucked_part in "${path[@]}"; do builtin printf 'path\0%s\0' "$__shucked_part"; done
        for __shucked_name in "${(@k)aliases}"; do builtin printf 'alias\0%s=%s\0' "$__shucked_name" "${aliases[$__shucked_name]}"; done
        for __shucked_name in "${(@k)functions}"; do builtin printf 'function\0%s\0' "$__shucked_name"; done
        builtin printf 'option\0aliases=%s\0' "${options[aliases]}"
        [[ -o histignorespace ]] && builtin printf 'ignore\0leading-space\0'
        (( HISTSIZE <= 0 )) && __shucked_private=1
        (( ${+functions[zshaddhistory]} || ${#zshaddhistory_functions} )) && __shucked_private=1
        [[ -n ${HISTORY_IGNORE-} ]] && __shucked_private=1
        builtin printf 'private\0%s\0' "$__shucked_private"
        if [[ -r ${SHUCKED_HISTORY_POLICY-} ]]; then
            { IFS= read -r __shucked_history_policy; IFS= read -r __shucked_files_policy; } < "$SHUCKED_HISTORY_POLICY"
            if [[ $__shucked_files_policy = 1 ]]; then
                local __shucked_history_file=${HISTFILE-}
                [[ -n $__shucked_history_file && $__shucked_history_file != /* ]] && __shucked_history_file=$PWD/$__shucked_history_file
                builtin printf 'history-file\0%s\0' "$__shucked_history_file"
            fi
        fi
        if [[ $__shucked_private = 0 && -r ${SHUCKED_HISTORY_POLICY-} ]] && IFS= read -r __shucked_history_policy < "$SHUCKED_HISTORY_POLICY" && [[ $__shucked_history_policy = 1 ]]; then
            builtin printf 'accepted-history\0%s\0' "$(builtin fc -ln -1 2>/dev/null)"
        fi
    } | ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_CAPTURE" "$__shucked_generation" "$$" zsh >/dev/null 2>&1
    return $__shucked_status
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __shucked_capture

if [[ ${SHUCKED_LIVE_ALLOWED-} = 1 ]]; then source "${SHUCKED_CAPTURE%/*}/live-zsh.zsh"; fi
