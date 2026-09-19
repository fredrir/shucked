# A reserved signal reads a private data request and forks current shell state.
# Editor words only populate completion variables; they never become a command.
__shucked_live_bash_complete() {
    builtin trap - DEBUG RETURN EXIT
    local __shucked_spec __shucked_function='' __shucked_word __shucked_quoted __shucked_i
    local -a __shucked_parts
    local -a COMP_WORDS=("${__shucked_fields[@]:4}" "${__shucked_fields[2]}")
    local COMP_CWORD=$((${#COMP_WORDS[@]} - 1)) COMP_LINE='' COMP_POINT COMP_TYPE=9 COMP_KEY=9
    local -a COMPREPLY=()
    if ((${#COMP_WORDS[@]} < 2)); then builtin printf 'P\0No argument completer selected\0E\0'; return; fi
    __shucked_spec=$(builtin complete -p -- "${COMP_WORDS[0]}" 2>/dev/null) || __shucked_spec=$(builtin complete -p -D 2>/dev/null)
    read -r -a __shucked_parts <<< "$__shucked_spec"
    for ((__shucked_i=0; __shucked_i<${#__shucked_parts[@]}-1; __shucked_i++)); do
        if [[ ${__shucked_parts[__shucked_i]} = -F ]]; then __shucked_function=${__shucked_parts[__shucked_i+1]}; break; fi
    done
    if [[ ! $__shucked_function =~ ^[a-zA-Z_][a-zA-Z_0-9.:+-]*$ ]] || ! builtin declare -F "$__shucked_function" >/dev/null; then
        builtin printf 'P\0The live completion registration is not a supported function callback\0E\0'; return
    fi
    for __shucked_word in "${COMP_WORDS[@]}"; do
        builtin printf -v __shucked_quoted '%q' "$__shucked_word"
        COMP_LINE+=${COMP_LINE:+ }$__shucked_quoted
    done
    COMP_POINT=${#COMP_LINE}
    "$__shucked_function" "${COMP_WORDS[0]}" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD-1]}" >/dev/null 2>&1
    __shucked_i=0
    for __shucked_word in "${COMPREPLY[@]}"; do
        ((__shucked_i++ >= 2000)) && { builtin printf 'P\0Live candidate limit reached\0'; break; }
        builtin printf 'M\0%s\0\0' "$__shucked_word"
    done
    builtin printf 'E\0'
}
__shucked_live_bash_request() {
    local __shucked_status=$? __shucked_value __shucked_pid
    local -a __shucked_fields=()
    while IFS= read -r -d '' __shucked_value; do __shucked_fields+=("$__shucked_value"); done < <(ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_READ")
    ((${#__shucked_fields[@]} >= 5)) || return "$__shucked_status"
    [[ ${__shucked_fields[0]} =~ ^[a-f0-9]{32}$ && ${__shucked_fields[1]} = "$__shucked_generation" ]] || return "$__shucked_status"
    (
      (
        # Job control is private to this fork and gives the callback its own group.
        set -m
        (
            set +m
            __shucked_live_bash_complete
        ) &
        __shucked_worker=$!
        ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "${SHUCKED_LIVE_RESULT%/*}/live-watchdog.cjs" "$__shucked_worker" &
        __shucked_watchdog=$!
        wait "$__shucked_worker"
        # A callback may return while its background descendants are still running.
        builtin kill -KILL -- "-$__shucked_worker" 2>/dev/null
        builtin kill "$__shucked_watchdog" 2>/dev/null
        wait "$__shucked_watchdog" 2>/dev/null
      ) | ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_RESULT" result "${__shucked_fields[0]}" "${__shucked_fields[1]}" 0
    ) </dev/null >/dev/null 2>&1 &
    __shucked_pid=$!
    builtin disown "$__shucked_pid" 2>/dev/null
    ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_RESULT" started "${__shucked_fields[0]}" "${__shucked_fields[1]}" "$__shucked_pid" </dev/null >/dev/null 2>&1
    return "$__shucked_status"
}
__shucked_live_signal=''
if [[ -z $(builtin trap -p USR1) ]]; then
    builtin trap '__shucked_live_bash_request >/dev/null 2>&1' USR1
    __shucked_live_signal=SIGUSR1
elif [[ -z $(builtin trap -p USR2) ]]; then
    builtin trap '__shucked_live_bash_request >/dev/null 2>&1' USR2
    __shucked_live_signal=SIGUSR2
fi
