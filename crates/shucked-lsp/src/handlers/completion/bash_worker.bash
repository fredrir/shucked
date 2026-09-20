# Persistent Bash host: initialize the completion framework once, pass requests as data.
unset BASH_ENV ENV PROMPT_COMMAND
shucked_initialized=0
declare -A shucked_loaded
_shucked_request() {
COMP_WORDS=("${shucked_words[@]}")
COMP_CWORD=$((${#COMP_WORDS[@]} - 1))
COMP_LINE=
for shucked_word in "${COMP_WORDS[@]}"; do
    printf -v shucked_quoted '%q' "$shucked_word"
    COMP_LINE+="${COMP_LINE:+ }$shucked_quoted"
done
COMP_POINT=${#COMP_LINE}
COMP_LINE+=$shucked_suffix
COMP_TYPE=9
COMP_KEY=9
COMPREPLY=()
shucked_command=${COMP_WORDS[0]##*/}
if [[ -z ${shucked_loaded[$shucked_command]+yes} ]]; then
  IFS=: read -r -a shucked_completion_dirs <<< "$SHUCKED_COMPLETION_PATHS"
  for shucked_completion_dir in "${shucked_completion_dirs[@]}"; do
    if [[ -f $shucked_completion_dir/$shucked_command ]]; then
      source "$shucked_completion_dir/$shucked_command" >/dev/null 2>&1
      break
    fi
  done
  complete -p -- "$shucked_command" >/dev/null 2>&1 || _comp_load -- "$shucked_command" >/dev/null 2>&1
  shucked_loaded[$shucked_command]=1
fi
read -r -a shucked_spec <<< "$(complete -p -- "$shucked_command" 2>/dev/null)"
shucked_function=
for ((shucked_i=0; shucked_i<${#shucked_spec[@]}; shucked_i++)); do
    if [[ ${shucked_spec[shucked_i]} == -F ]]; then
        shucked_function=${shucked_spec[shucked_i+1]}
        break
    fi
done
[[ $shucked_function =~ ^[a-zA-Z_][a-zA-Z_0-9:.-]*$ ]] || { printf 'E\000'; return; }
declare -F -- "$shucked_function" >/dev/null || { printf 'E\000'; return; }
# Readline quotes filename candidates itself. Other callbacks supply shell-word
# insertion text, which can include an unquoted completion delimiter.
shucked_filenames=0
shucked_noquote=0
shucked_nospace=0
for ((shucked_i=0; shucked_i<${#shucked_spec[@]}-1; shucked_i++)); do
    if [[ ${shucked_spec[shucked_i]} == -o ]]; then
        case ${shucked_spec[shucked_i+1]} in
            filenames) shucked_filenames=1 ;;
            noquote) shucked_noquote=1 ;;
            nospace) shucked_nospace=1 ;;
        esac
    fi
done
compopt() {
    local shucked_mode shucked_option
    while (($#)); do
        shucked_mode=$1; shift
        case $shucked_mode in
            -o|+o)
                (($#)) || return 1
                shucked_option=$1; shift
                case $shucked_option in
                    filenames) [[ $shucked_mode == -o ]] && shucked_filenames=1 || shucked_filenames=0 ;;
                    noquote) [[ $shucked_mode == -o ]] && shucked_noquote=1 || shucked_noquote=0
shucked_nospace=0 ;;
                    nospace) [[ $shucked_mode == -o ]] && shucked_nospace=1 || shucked_nospace=0 ;;
                    bashdefault|default|dirnames|nosort|plusdirs) ;;
                    *) return 1 ;;
                esac
                ;;
            -D|-E|-I) ;;
            *) return 1 ;;
        esac
    done
    return 0
}
"$shucked_function" "${COMP_WORDS[0]}" "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD-1]}" >/dev/null 2>&1
for shucked_candidate in "${COMPREPLY[@]:0:2000}"; do
    if ((shucked_filenames && !shucked_noquote)); then
        printf 'C\000%s\000\000file\000%s\000' "$shucked_candidate" "$shucked_nospace"
    else
        printf 'Q\000%s\000\000\000%s\000' "$shucked_candidate" "$shucked_nospace"
    fi
done
printf 'E\000'

}
while IFS= read -r -d '' shucked_directory && IFS= read -r -d '' shucked_path && IFS= read -r -d '' shucked_suffix && IFS= read -r -d '' shucked_count; do
  [[ $shucked_count =~ ^[0-9]+$ && $shucked_count -le 257 ]] || exit 1
  shucked_words=()
  for ((shucked_i=0; shucked_i<shucked_count; shucked_i++)); do
    IFS= read -r -d '' shucked_word || exit 0
    shucked_words+=("$shucked_word")
  done
  printf 'P\0000\000'
  builtin cd -- "$shucked_directory" 2>/dev/null || { printf 'E\000'; continue; }
  export PATH=$shucked_path
  hash -r
  if (( ! shucked_initialized )); then
# The command line arrives as positional data, never as a shell program.
unset BASH_ENV ENV PROMPT_COMMAND
export BASH_COMPLETION_USER_FILE=/dev/null
export BASH_COMPLETION_COMPAT_DIR="$SHUCKED_PROVIDER_ROOT/disabled"
export BASH_COMPLETION_USER_DIR="$SHUCKED_PROVIDER_ROOT/packs/bash-completion"
source "$SHUCKED_PROVIDER_ROOT/packs/bash-completion/bash_completion" >/dev/null 2>&1 || exit 1

    shucked_initialized=1
  fi
  _shucked_request
 done
