# Definitions are streamed directly into a private worker, never stored in files.
function __shucked_live_fish_state
    # Event handlers must not run while private state is reconstructed.
    set -l handlers
    for line in (functions --handlers)
        string match -q 'Event *' -- "$line"; and continue
        set -l fields (string split --no-empty ' ' -- "$line")
        test (count $fields) -ge 2; and set -a handlers "$fields[-1]"
    end
    set -l count 0
    for name in (functions --all --names)
        string match -q '__shucked*' -- "$name"; and continue
        string match -q 'SHUCKED_*' -- "$name"; and continue
        set count (math $count + 1); if test $count -gt 2000; builtin printf '# __shucked_live_state_truncated\n'; break; end
        if contains -- "$name" $handlers
            builtin printf '# __shucked_live_state_truncated\n'
            continue
        end
        functions -- "$name"
    end
    complete
    set count 0
    for name in (set --global --names)
        string match -q '__shucked*' -- "$name"; and continue
        string match -q 'SHUCKED_*' -- "$name"; and continue
        string match -rq '^[a-zA-Z_][a-zA-Z_0-9]*$' -- "$name"; or continue
        contains -- "$name" fish_pid status pipestatus version SHLVL PWD; and continue
        set count (math $count + 1); if test $count -gt 512; builtin printf '# __shucked_live_state_truncated\n'; break; end
        builtin printf 'set -g -- %s' "$name"
        for value in $$name
            builtin printf ' %s' (string escape -- "$value")
        end
        builtin printf '\n'
    end
end
function __shucked_live_fish_request
    set -l fields
    env ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_READ" | while read --null -l field
        set -a fields "$field"
    end
    test (count $fields) -ge 5; or return
    string match -rq '^[a-f0-9]{32}$' -- "$fields[1]"; or return
    test "$fields[2]" = "$__shucked_generation"; or return
    __shucked_live_fish_state | env ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_FISH" "$fields[1]" "$fields[2]" (status fish-path) "$fields[3]" $fields[5..] >/dev/null 2>&1 &
    set -l pid $last_pid
    disown $pid 2>/dev/null
    env ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_RESULT" started "$fields[1]" "$fields[2]" "$pid" </dev/null >/dev/null 2>&1
end
set -g __shucked_live_signal ''
# Fish allows multiple signal handlers; reserve a signal only when none exist.
if test -z (functions --handlers-type signal | string collect)
    function __shucked_live_fish_signal --on-signal USR1
        __shucked_live_fish_request >/dev/null 2>&1
    end
    set -g __shucked_live_signal SIGUSR1
end
