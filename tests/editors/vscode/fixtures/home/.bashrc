alias shucked_smoke_alias='printf'
HISTFILE=$HOME/custom_history
HISTSIZE=1000
SAVEHIST=1000
# A custom completer that reads current, non-exported shell state.
my_completion_value=live_fixture_value
custom_fixture() { :; }
_custom_fixture() { COMPREPLY=("$my_completion_value"); }
complete -F _custom_fixture custom_fixture
