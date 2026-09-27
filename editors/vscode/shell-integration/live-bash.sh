# A reserved signal makes the shell read a private request and fork its current
# completion state. Editor words only populate completion variables; they never
# become a command. A persistent helper (live-helper.cjs, started below) writes
# each request, sends the signal, reads the worker's records from a FIFO it
# owns, and stops workers that outlive their deadline: no process is started
# per request beyond the forked worker itself.
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
    # Readline quotes filename candidates itself. Other callbacks supply shell-word
    # insertion text, which can include an unquoted completion delimiter.
    __shucked_filenames=0
    __shucked_noquote=0
    for ((__shucked_i=0; __shucked_i<${#__shucked_parts[@]}-1; __shucked_i++)); do
        if [[ ${__shucked_parts[__shucked_i]} == -o ]]; then
            case ${__shucked_parts[__shucked_i+1]} in
                filenames) __shucked_filenames=1 ;;
                noquote) __shucked_noquote=1 ;;
            esac
        fi
    done
    compopt() {
        local __shucked_mode __shucked_option
        while (($#)); do
            __shucked_mode=$1; shift
            case $__shucked_mode in
                -o|+o)
                    (($#)) || return 1
                    __shucked_option=$1; shift
                    case $__shucked_option in
                        filenames) [[ $__shucked_mode == -o ]] && __shucked_filenames=1 || __shucked_filenames=0 ;;
                        noquote) [[ $__shucked_mode == -o ]] && __shucked_noquote=1 || __shucked_noquote=0 ;;
                        bashdefault|default|dirnames|nosort|nospace|plusdirs) ;;
                        *) return 1 ;;
                    esac
                    ;;
                -D|-E|-I) ;;
                *) return 1 ;;
            esac
        done
        return 0
    }
    "$__shucked_function" "${COMP_WORDS[0]}" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD-1]}" >/dev/null 2>&1
    __shucked_i=0
    for __shucked_word in "${COMPREPLY[@]}"; do
        ((__shucked_i++ >= 2000)) && { builtin printf 'P\0Live candidate limit reached\0'; break; }
        if ((__shucked_filenames && !__shucked_noquote)); then
            builtin printf 'M\0%s\0\0' "$__shucked_word"
        else
            builtin printf 'B\0%s\0\0' "$__shucked_word"
        fi
    done
    builtin printf 'E\0'
}
__shucked_live_bash_request() {
    local __shucked_status=$? __shucked_value
    local -a __shucked_fields=()
    [[ -f $SHUCKED_LIVE_DIRECTORY/request && -p $SHUCKED_LIVE_DIRECTORY/live.fifo ]] || return "$__shucked_status"
    while IFS= read -r -d '' __shucked_value; do __shucked_fields+=("$__shucked_value"); done < "$SHUCKED_LIVE_DIRECTORY/request"
    ((${#__shucked_fields[@]} >= 5)) || return "$__shucked_status"
    [[ ${__shucked_fields[0]} =~ ^[a-f0-9]{32}$ && ${__shucked_fields[1]} = "$__shucked_generation" ]] || return "$__shucked_status"
    # A signal delivered twice for one request must not start a second worker.
    [[ ${__shucked_fields[0]} != "$__shucked_live_served" ]] || return "$__shucked_status"
    __shucked_live_served=${__shucked_fields[0]}
    (
        # Job control is private to this fork and gives the worker its own process
        # group, which the helper stops at the deadline or on cancellation.
        set -m
        (
            set +m
            builtin printf 'R\0%s\0%s\0' "${__shucked_fields[0]}" "$BASHPID"
            __shucked_live_bash_complete
        ) > "$SHUCKED_LIVE_DIRECTORY/live.fifo" &
        wait "$!"
        # A callback may return while its background descendants are still running.
        builtin kill -KILL -- "-$!" 2>/dev/null
    ) </dev/null >/dev/null 2>&1 &
    builtin disown "$!" 2>/dev/null
    return "$__shucked_status"
}
__shucked_live_signal=''
__shucked_live_served=''
# Readline runs a pending trap promptly only for the signals it watches itself;
# any other signal waits for the next keystroke, which an editor never sends.
# The window-size signal is the one of those that stays silent when the size
# is unchanged, so it carries the requests. The helper needs BASHPID and named
# file descriptors (bash 4.1).
if [[ -n ${SHUCKED_LIVE_HELPER-} && -n ${SHUCKED_LIVE_DIRECTORY-} && -z ${__shucked_live_helper_fd-} ]] \
    && (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 1) )); then
    if [[ -z $(builtin trap -p WINCH) ]]; then
        builtin trap '__shucked_live_bash_request >/dev/null 2>&1' WINCH
        __shucked_live_signal=SIGWINCH
    fi
    if [[ -n $__shucked_live_signal ]]; then
        # The helper reads its stdin from a pipe only this shell writes to, so it
        # sees end-of-file the moment the shell exits; it also watches its parent.
        builtin eval 'exec {__shucked_live_helper_fd}> >(ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_HELPER" bash "$$" "$__shucked_live_signal" >/dev/null 2>&1)'
        __shucked_live_helper_pid=$!
    fi
fi
