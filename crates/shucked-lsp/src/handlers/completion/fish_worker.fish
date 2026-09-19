# No personal configuration, universal-variable paths, or editor evaluation.
set -g fish_function_path "$SHUCKED_PROVIDER_ROOT/packs/fish/share/functions" "$__fish_data_dir/functions"
set -g fish_complete_path "$SHUCKED_PROVIDER_ROOT/packs/fish/share/completions"
set -l shucked_buffer (string join ' ' -- (string escape -- $argv))
printf 'P\0000\000'
set -l shucked_count 0
for shucked_candidate in (complete --do-complete "$shucked_buffer")
    set shucked_count (math "$shucked_count + 1")
    test "$shucked_count" -le 2000; or break
    set -l shucked_fields (string split -m 1 \t -- "$shucked_candidate")
    set -l shucked_text (string unescape -- "$shucked_fields[1]")
    printf 'M\000%s\000%s\000' "$shucked_text" "$shucked_fields[2]"
end
printf 'E\000'
