emulate -L zsh
zmodload zsh/zpty || exit 1
zmodload zsh/zselect || exit 1
exec 3>&1
# Only this fixed program reaches zpty's command evaluator. Editor text is data.
zpty -b shucked 'zmodload zsh/system; print -rn -u3 -- P$'"'\0'"'${sysparams[pid]}$'"'\0'"'; if [[ $SHUCKED_NATIVE_PERSONAL == 1 ]]; then exec "$SHUCKED_NATIVE_SHELL" -l -i -c "$SHUCKED_NATIVE_SCRIPT"; else exec "$SHUCKED_NATIVE_SHELL" -f -i -c "$SHUCKED_NATIVE_SCRIPT"; fi' || exit 1
trap 'zpty -d shucked 2>/dev/null' EXIT HUP TERM
typeset shucked_tail=""
typeset -i shucked_ready=0
while zpty -t shucked; do
    zpty -r shucked shucked_output
    shucked_tail+=$shucked_output
    shucked_tail=${shucked_tail[-512,-1]}
    if (( ! shucked_ready )) && [[ $shucked_tail == *SHUCKED_NATIVE_READY* ]]; then
        shucked_ready=1
        zpty -w -n shucked $'\t'
    fi
    [[ $shucked_tail == *SHUCKED_NATIVE_DONE* ]] && break
    zselect -t 1
done
