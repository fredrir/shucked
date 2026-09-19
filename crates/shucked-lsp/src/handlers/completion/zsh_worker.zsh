# Managed completion session; personal startup files require explicit opt-in.
builtin unsetopt xtrace verbose
exec 2>/dev/null
if [[ -d $SHUCKED_PROVIDER_ROOT/packs/zsh ]]; then
  fpath=("$SHUCKED_PROVIDER_ROOT"/packs/zsh-completions/src
         "$SHUCKED_PROVIDER_ROOT"/packs/zsh/Completion
         "$SHUCKED_PROVIDER_ROOT"/packs/zsh/Completion/**/(N/)
         $fpath)
fi
autoload -Uz compinit
(($+functions[compdef])) || compinit -i -D
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
  exit 0
}

zle -C _shucked_native_complete complete-word _shucked_native_complete
bindkey -e
bindkey '^I' _shucked_native_complete
typeset shucked_buffer=$SHUCKED_NATIVE_BUFFER
function _shucked_ready { builtin print -r -- SHUCKED_NATIVE_READY; }
zle -N zle-line-init _shucked_ready
vared shucked_buffer
