emulate -L zsh
zmodload zsh/zpty || exit 1
zmodload zsh/zselect || exit 1
exec 3>&1 4<&0
# Only the fixed worker program reaches the evaluator. Buffer text stays on FD 4.
zpty -b shucked 'if [[ $SHUCKED_NATIVE_PERSONAL == 1 ]]; then exec "$SHUCKED_NATIVE_SHELL" -l -i -c "$SHUCKED_NATIVE_SCRIPT"; else exec "$SHUCKED_NATIVE_SHELL" -f -i -c "$SHUCKED_NATIVE_SCRIPT"; fi' || exit 1
trap 'zpty -d shucked 2>/dev/null' EXIT HUP TERM
typeset shucked_tail=""
while zpty -t shucked; do
  zpty -r shucked shucked_output
  shucked_tail+=$shucked_output
  shucked_tail=${shucked_tail[-512,-1]}
  if [[ $shucked_tail == *SHUCKED_NATIVE_READY* ]]; then
    shucked_tail=""
    zpty -w -n shucked $'\t'
  fi
  if [[ $shucked_tail == *SHUCKED_NATIVE_DONE* ]]; then
    shucked_tail=""
    zpty -w -n shucked $'\r'
  fi
  zselect -t 1
done
exit 0
