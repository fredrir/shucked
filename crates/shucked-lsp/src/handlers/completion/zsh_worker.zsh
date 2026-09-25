# Persistent managed completion host. Editor buffers travel over a separate data FD.
builtin unsetopt xtrace verbose
exec 2>/dev/null
zmodload zsh/system || exit 1
IFS= read -r -d $'\0' -u4 shucked_directory || exit 0
IFS= read -r -d $'\0' -u4 shucked_path || exit 0
IFS= read -r -d $'\0' -u4 shucked_buffer || exit 0
IFS= read -r -d $'\0' -u4 shucked_cursor || exit 0
builtin print -rn -u3 -- P$'\0'"${sysparams[pid]}"$'\0'
typeset -i shucked_first=1
if [[ -d $SHUCKED_PROVIDER_ROOT/packs/zsh ]]; then
  fpath=("$SHUCKED_PROVIDER_ROOT"/packs/zsh-extra
         "$SHUCKED_PROVIDER_ROOT"/packs/zsh-completions/src
         "$SHUCKED_PROVIDER_ROOT"/packs/zsh/Completion
         "$SHUCKED_PROVIDER_ROOT"/packs/zsh/Completion/**/(N/)
         $fpath)
fi
[[ -n $SHUCKED_COMPLETION_PATHS ]] && fpath=("${(@s/:/)SHUCKED_COMPLETION_PATHS}" $fpath)
autoload -Uz compinit
# Native execution authorizes these provider roots, including read-only bundles
# mounted with a different owner inside a container. The server keys the dump
# file by the function path, so an existing dump is trusted without a rescan.
if (( ! $+functions[compdef] )); then
  if [[ -n $SHUCKED_COMPDUMP ]]; then
    compinit -u -C -d "$SHUCKED_COMPDUMP"
  else
    compinit -u -D
  fi
fi
zmodload zsh/zutil || exit 1
function compadd {
  local -A shucked_options
  local -a shucked_matches shucked_descriptions
  local shucked_description_name shucked_index shucked_match shucked_description shucked_kind shucked_suffix shucked_no_space shucked_record
  zparseopts -E -A shucked_options d: P: S: p: s: i: I: W: X: x: f n Q O: A: D:
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
    shucked_suffix=${shucked_options[-S]}
    shucked_no_space=${+shucked_options[-S]}
    if [[ $shucked_suffix == ' ' ]]; then
      shucked_suffix=''
      shucked_no_space=0
    fi
    shucked_match="${IPREFIX}${shucked_options[-i]}${shucked_options[-P]}${shucked_options[-p]}${shucked_matches[shucked_index]}${shucked_options[-s]}${shucked_suffix}${shucked_options[-I]}${ISUFFIX}"
    shucked_description=${shucked_descriptions[shucked_index]:-${shucked_options[-X]}}
    shucked_kind=''
    if (( ${+shucked_options[-f]} )); then
      shucked_kind=file
      if [[ -d ${shucked_options[-W]}${shucked_match} ]]; then
        shucked_kind=directory
        [[ $shucked_match != */ ]] && shucked_match+=/
      fi
    fi
    shucked_record=C
    (( ${+shucked_options[-Q]} )) && shucked_record=Q
    builtin print -rn -u3 -- "$shucked_record"$'\0'"$shucked_match"$'\0'"$shucked_description"$'\0'"$shucked_kind"$'\0'"$shucked_no_space"$'\0'
  done
  builtin compadd "$@"
}

function _shucked_native_complete {
  local -i shucked_count=0
  COLUMNS=240 LINES=40
  unset 'compstate[vared]'
  # Preserve matching styles but request descriptions alongside each candidate.
  zstyle ':completion:*' verbose yes
  zstyle ':completion:*' list-grouped false
  zstyle ':completion:*' format '%d'
  _main_complete
  builtin print -rn -u3 -- E$'\0'
  builtin print -r -- SHUCKED_NATIVE_DONE
  compstate[insert]=''
  compstate[list]=''
}

zle -C _shucked_native_complete complete-word _shucked_native_complete
bindkey -e
bindkey '^I' _shucked_native_complete
function _shucked_ready { CURSOR=$shucked_cursor; builtin print -r -- SHUCKED_NATIVE_READY; }
zle -N zle-line-init _shucked_ready
while true; do
  if (( shucked_first )); then
    shucked_first=0
  else
    IFS= read -r -d $'\0' -u4 shucked_directory || break
    IFS= read -r -d $'\0' -u4 shucked_path || break
    IFS= read -r -d $'\0' -u4 shucked_buffer || break
    IFS= read -r -d $'\0' -u4 shucked_cursor || break
    builtin print -rn -u3 -- P$'\0'"${sysparams[pid]}"$'\0'
  fi
  builtin cd -- "$shucked_directory" 2>/dev/null || { builtin print -rn -u3 -- E$'\0'; continue; }
  export PATH=$shucked_path
  rehash
  vared shucked_buffer
 done
