alias shucked_smoke_alias='printf'
HISTFILE=$HOME/custom_history
HISTSIZE=1000
SAVEHIST=1000
autoload -Uz compinit; compinit -D
# A custom completer that reads current, non-exported shell state.
my_completion_value=live_fixture_value
custom_fixture() { :; }
_custom_fixture() { compadd -- "$my_completion_value"; }
compdef _custom_fixture custom_fixture
# A completer that never finishes on its own; the editor must stop its worker.
slow_fixture() { :; }
_slow_fixture() { zmodload zsh/system; printf '%s' "$sysparams[pid]" > "$HOME/live_worker_pid"; sleep 10; compadd -- live_slow; }
compdef _slow_fixture slow_fixture
