# Sourced once in an explicitly attached interactive Fish session.
set -g __shucked_generation 0
function __shucked_capture --on-event fish_prompt
    set -l __shucked_previous_status $status
    set -g __shucked_generation (math $__shucked_generation + 1)
    begin
        builtin printf 'cwd\0%s\0' "$PWD"
        for entry in $PATH
            builtin printf 'path\0%s\0' "$entry"
        end
        functions --names | while read -l name
            builtin printf 'function\0%s\0' "$name"
        end
        builtin printf 'ignore\0leading-space\0'
        if functions -q fish_should_add_to_history
            builtin printf 'private\0%s\0' 1
        end
        if set -q fish_private_mode; or begin; set -q fish_history; and test "$fish_history" = ''; end
            builtin printf 'private\0%s\0' 1
        end
    end | env ELECTRON_RUN_AS_NODE=1 "$SHUCKED_NODE" "$SHUCKED_CAPTURE" "$__shucked_generation" "$fish_pid" fish >/dev/null 2>&1
    return $__shucked_previous_status
end
