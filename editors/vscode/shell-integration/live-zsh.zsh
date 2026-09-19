# Forked Zsh workers inherit current functions, styles, options and completion maps.
__shucked_live_zsh_child() {
  precmd_functions=()
  __shucked_capture() { :; }
  unfunction TRAPEXIT TRAPDEBUG TRAPZERR 2>/dev/null
  trap - EXIT DEBUG ZERR
  exec 2>/dev/null
  # zpty creates a separate session and process group. Keep callback children in it.
  unsetopt monitor
  zmodload zsh/system || exit 1
  local __shucked_watchfd
  local __shucked_group=$sysparams[pid]
  exec {__shucked_watchfd}> >(ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "${SHUCKED_LIVE_RESULT:h}/live-watchdog.cjs" "$__shucked_group" --lifetime-pipe)
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
__shucked_live_zsh_complete() {
  (( $+functions[compdef] )) || { builtin print -rn -- P$'\0'"The session has no initialized completion system"$'\0'E$'\0'; return; }
  zmodload zsh/zpty || return
  zmodload zsh/zselect || return
  exec 3>&1
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
  local __shucked_value __shucked_pid
  local -a __shucked_fields=()
  while IFS= read -r -d '' __shucked_value; do __shucked_fields+=("$__shucked_value"); done < <(ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_READ")
  (( ${#__shucked_fields[@]} >= 5 )) || return 0
  [[ ${__shucked_fields[1]} =~ '^[a-f0-9]{32}$' && ${__shucked_fields[2]} == "$__shucked_generation" ]] || return 0
  (
    __shucked_live_zsh_complete | ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_RESULT" result "${__shucked_fields[1]}" "${__shucked_fields[2]}" 0
  ) </dev/null >/dev/null 2>&1 &!
  __shucked_pid=$!
  ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_RESULT" started "${__shucked_fields[1]}" "${__shucked_fields[2]}" "$__shucked_pid" </dev/null >/dev/null 2>&1
  return 0
}
typeset __shucked_live_signal=''
if [[ -z $(builtin trap -p USR1) ]] && (( ! $+functions[TRAPUSR1] )); then
  TRAPUSR1() { __shucked_live_zsh_request >/dev/null 2>&1; }
  __shucked_live_signal=SIGUSR1
elif [[ -z $(builtin trap -p USR2) ]] && (( ! $+functions[TRAPUSR2] )); then
  TRAPUSR2() { __shucked_live_zsh_request >/dev/null 2>&1; }
  __shucked_live_signal=SIGUSR2
fi
