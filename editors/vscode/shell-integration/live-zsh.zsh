# Forked Zsh workers inherit current functions, styles, options and completion
# maps. A persistent helper (live-helper.cjs, started below) writes each
# request, sends the reserved signal, reads the worker's records from a FIFO it
# owns, and stops workers that outlive their deadline.
__shucked_live_zsh_child() {
  precmd_functions=()
  __shucked_capture() { :; }
  unfunction TRAPEXIT TRAPDEBUG TRAPZERR 2>/dev/null
  trap - EXIT DEBUG ZERR
  exec 2>/dev/null
  # zpty creates a separate session and process group. Keep callback children in it.
  unsetopt monitor
  zmodload zsh/system || exit 1
  # This session's group and the wrapper that drives the pty are what the helper stops.
  builtin print -rn -u3 -- R$'\0'"${__shucked_fields[1]}"$'\0'"$sysparams[pid],$sysparams[ppid]"$'\0'
  zmodload zsh/zutil || exit 1
function compadd {
  local -A shucked_options
  local -a shucked_matches shucked_descriptions
  local shucked_description_name shucked_index shucked_match shucked_description
  zparseopts -E -A shucked_options d: P: S: p: s: i: I: W: X: x: f n O: A: D:
  # Internal filtering calls must preserve the completion system's arrays.
  if [[ -n ${shucked_options[-O]}${shucked_options[-A]}${shucked_options[-D]} ]]; then
    builtin compadd "$@"
    return
  fi
  shucked_description_name=${shucked_options[-d]}
  [[ -n $shucked_description_name ]] && shucked_descriptions=("${(@P)shucked_description_name}")
  builtin compadd -A shucked_matches -D shucked_descriptions "$@"
  for ((shucked_index = 1; shucked_index <= $#shucked_matches; shucked_index++)); do
    ((++shucked_count > 2000)) && break
    shucked_match="${IPREFIX}${shucked_options[-i]}${shucked_options[-P]}${shucked_options[-p]}${shucked_matches[shucked_index]}${shucked_options[-s]}${shucked_options[-S]}${shucked_options[-I]}${ISUFFIX}"
    shucked_description=${shucked_descriptions[shucked_index]:-${shucked_options[-X]}}
    if [[ -n ${shucked_options[(I) - f]} && -d ${shucked_options[-W]}${shucked_match} && $shucked_match != */ ]]; then
      shucked_match+=/
    fi
    builtin print -rn -u3 -- M$'\0'"$shucked_match"$'\0'"$shucked_description"$'\0'
  done
  builtin compadd "$@"
}

  __shucked_live_widget() {
    local -i shucked_count=0
    unset 'compstate[vared]'
    _main_complete
    builtin print -rn -u3 -- E$'\0'
    builtin print -r -- SHUCKED_LIVE_DONE
    exit 0
  }
  zle -C __shucked_live_widget complete-word __shucked_live_widget
  bindkey -e
  bindkey '^I' __shucked_live_widget
  __shucked_live_ready() { builtin print -r -- SHUCKED_LIVE_READY; }
  zle -N zle-line-init __shucked_live_ready
  local __shucked_buffer='' __shucked_word
  local -a __shucked_words=("${__shucked_fields[@]:4}" "${__shucked_fields[3]}")
  for __shucked_word in "${__shucked_words[@]}"; do
    __shucked_buffer+=${__shucked_buffer:+ }${(q)__shucked_word}
  done
  vared __shucked_buffer
}
# Runs in a forked subshell whose fd 3 is the helper's FIFO.
__shucked_live_zsh_complete() {
  (( $+functions[compdef] )) || {
    builtin print -rn -u3 -- R$'\0'"${__shucked_fields[1]}"$'\0'0$'\0'P$'\0'"The session has no initialized completion system"$'\0'E$'\0'
    return
  }
  zmodload zsh/zpty || return
  zmodload zsh/zselect || return
  zpty -b __shucked_live __shucked_live_zsh_child || return
  trap 'zpty -d __shucked_live 2>/dev/null' EXIT HUP TERM
  local __shucked_output __shucked_tail=''
  local -i __shucked_ready=0
  while zpty -t __shucked_live; do
    zpty -r __shucked_live __shucked_output
    __shucked_tail+=$__shucked_output
    __shucked_tail=${__shucked_tail[-512,-1]}
    if (( ! __shucked_ready )) && [[ $__shucked_tail == *SHUCKED_LIVE_READY* ]]; then
      __shucked_ready=1
      zpty -w -n __shucked_live $'\t'
    fi
    [[ $__shucked_tail == *SHUCKED_LIVE_DONE* ]] && break
    zselect -t 1
  done
}
__shucked_live_zsh_request() {
  local __shucked_value
  local -a __shucked_fields=()
  [[ -f $SHUCKED_LIVE_DIRECTORY/request && -p $SHUCKED_LIVE_DIRECTORY/live.fifo ]] || return 0
  while IFS= read -r -d '' __shucked_value; do __shucked_fields+=("$__shucked_value"); done < "$SHUCKED_LIVE_DIRECTORY/request"
  (( ${#__shucked_fields[@]} >= 5 )) || return 0
  [[ ${__shucked_fields[1]} =~ '^[a-f0-9]{32}$' && ${__shucked_fields[2]} == "$__shucked_generation" ]] || return 0
  # A signal delivered twice for one request must not start a second worker.
  [[ ${__shucked_fields[1]} != "$__shucked_live_served" ]] || return 0
  __shucked_live_served=${__shucked_fields[1]}
  (
    exec 3>"$SHUCKED_LIVE_DIRECTORY/live.fifo"
    __shucked_live_zsh_complete
  ) </dev/null >/dev/null 2>&1 &!
  return 0
}
typeset __shucked_live_signal='' __shucked_live_served=''
if [[ -n ${SHUCKED_LIVE_HELPER-} && -n ${SHUCKED_LIVE_DIRECTORY-} && -z ${__shucked_live_helper_fd-} ]]; then
  if [[ -z $(builtin trap -p USR1) ]] && (( ! $+functions[TRAPUSR1] )); then
    TRAPUSR1() { __shucked_live_zsh_request >/dev/null 2>&1; }
    __shucked_live_signal=SIGUSR1
  elif [[ -z $(builtin trap -p USR2) ]] && (( ! $+functions[TRAPUSR2] )); then
    TRAPUSR2() { __shucked_live_zsh_request >/dev/null 2>&1; }
    __shucked_live_signal=SIGUSR2
  fi
  if [[ -n $__shucked_live_signal ]]; then
    # The helper reads its stdin from a pipe only this shell writes to, so it
    # sees end-of-file the moment the shell exits; it also watches its parent.
    exec {__shucked_live_helper_fd}> >(ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_HELPER" zsh "$$" "$__shucked_live_signal" >/dev/null 2>&1)
  fi
fi
