# Definitions are streamed into a persistent helper (live-helper.cjs, started
# below) over a private FIFO, never stored in files. The helper runs them in a
# private fish together with the completion command and reports the result.
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
    set -l request "$SHUCKED_LIVE_DIRECTORY/request"
    set -l fifo "$SHUCKED_LIVE_DIRECTORY/live.fifo"
    test -f "$request"; and test -p "$fifo"; or return
    set -l fields
    while read --null -l field
        set -a fields "$field"
    end < "$request"
    test (count $fields) -ge 5; or return
    string match -rq '^[a-f0-9]{32}$' -- "$fields[1]"; or return
    test "$fields[2]" = "$__shucked_generation"; or return
    # A signal delivered twice for one request must not start a second worker.
    test "$fields[1]" != "$__shucked_live_served"; or return
    set -g __shucked_live_served "$fields[1]"
    # Each editor word is escaped before the private fish sees it; nothing is evaluated here.
    set -l line (string escape -- $fields[5..] "$fields[3]" | string join ' ')
    begin
        builtin printf 'R\0%s\0%s\0F\0' "$fields[1]" 0
        __shucked_live_fish_state
        builtin printf 'complete -C %s\n' (string escape -- "$line")
        builtin printf '\0E\0'
    end > "$fifo" &
    disown 2>/dev/null
end
set -g __shucked_live_signal ''
set -g __shucked_live_served ''
if set -q SHUCKED_LIVE_HELPER; and set -q SHUCKED_LIVE_DIRECTORY; and not set -q __shucked_live_helper_pid
    # Fish allows multiple signal handlers; reserve a signal only when none exist.
    if test -z (functions --handlers-type signal | string collect)
        function __shucked_live_fish_signal --on-signal USR1
            __shucked_live_fish_request >/dev/null 2>&1
        end
        set -g __shucked_live_signal SIGUSR1
    end
    if test -n "$__shucked_live_signal"
        # The helper watches its parent and exits when this shell does.
        env ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_LIVE_HELPER" fish $fish_pid "$__shucked_live_signal" (status fish-path) </dev/null >/dev/null 2>&1 &
        set -g __shucked_live_helper_pid $last_pid
        disown $__shucked_live_helper_pid 2>/dev/null
    end
end
