# Sourced once in an explicitly attached interactive Fish session.
set -g __shucked_generation 0
function __shucked_capture --on-event fish_prompt
    set -l __shucked_previous_status $status
    set -g __shucked_generation (math $__shucked_generation + 1)
    set -l __shucked_private 0
    begin
        builtin printf 'cwd\0%s\0' "$PWD"
        if set -q __shucked_live_signal; and test "$__shucked_live_signal" = SIGUSR1
            set -l handlers (functions --handlers-type signal)
            if test (count $handlers) = 2; and string match -q '*__shucked_live_fish_signal' -- "$handlers[2]"
                builtin printf 'live-signal\0SIGUSR1\0'
            end
        end
        for entry in $PATH
            builtin printf 'path\0%s\0' "$entry"
        end
        functions --names | while read -l name
            builtin printf 'function\0%s\0' "$name"
        end
        builtin printf 'ignore\0leading-space\0'
        if functions -q fish_should_add_to_history
            set __shucked_private 1
        end
        if set -q fish_private_mode; or begin; set -q fish_history; and test "$fish_history" = ''; end
            set __shucked_private 1
        end
        builtin printf 'private\0%s\0' "$__shucked_private"
        if set -q SHUCKED_HISTORY_POLICY; and test -r "$SHUCKED_HISTORY_POLICY"
            set -l __shucked_history_policy ''; set -l __shucked_files_policy ''
            begin; read __shucked_history_policy; read __shucked_files_policy; end < "$SHUCKED_HISTORY_POLICY"
            if test "$__shucked_files_policy" = 1
                set -l __shucked_history_name fish
                if set -q fish_history; set __shucked_history_name "$fish_history"; end
                set -l __shucked_data_home "$HOME/.local/share"
                if set -q XDG_DATA_HOME; set __shucked_data_home "$XDG_DATA_HOME"; end
                set -l __shucked_history_file ''
                if test -n "$__shucked_history_name"; set __shucked_history_file "$__shucked_data_home/fish/"$__shucked_history_name"_history"; end
                builtin printf 'history-file\0%s\0' "$__shucked_history_file"
            end
        end
        if test $__shucked_private = 0; and set -q SHUCKED_HISTORY_POLICY; and test -r "$SHUCKED_HISTORY_POLICY"
            read -l __shucked_history_policy < "$SHUCKED_HISTORY_POLICY"
            if test "$__shucked_history_policy" = 1
                builtin printf 'accepted-history\0%s\0' (history --max=1 | string collect)
            end
        end
    end | env ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_CAPTURE" "$__shucked_generation" "$fish_pid" fish >/dev/null 2>&1
    return $__shucked_previous_status
end

if test "$SHUCKED_LIVE_ALLOWED" = 1
    source (string replace -r '/[^/]*$' '/live-fish.fish' -- "$SHUCKED_CAPTURE")
end
