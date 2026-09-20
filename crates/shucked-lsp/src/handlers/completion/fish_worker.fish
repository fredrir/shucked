# Persistent Fish host with a NUL-delimited data protocol.
# No personal configuration, universal-variable paths, or editor evaluation.
set -g fish_function_path "$SHUCKED_PROVIDER_ROOT/packs/fish/share/functions" "$__fish_data_dir/functions"
set -g fish_complete_path (string split : -- "$SHUCKED_COMPLETION_PATHS") "$SHUCKED_PROVIDER_ROOT/packs/fish/share/completions"

while read --null shucked_directory; and read --null shucked_path; and read --null shucked_suffix; and read --null shucked_count
    string match --quiet --regex '^[0-9]+$' -- "$shucked_count"; or exit 1
    test "$shucked_count" -le 257; or exit 1
    set -l shucked_words
    for shucked_index in (seq "$shucked_count")
        read --null shucked_word; or exit 0
        set -a shucked_words "$shucked_word"
    end
    printf 'P\0000\000'
    builtin cd -- "$shucked_directory" 2>/dev/null; or begin; printf 'E\000'; continue; end
    set -gx PATH (string split : -- "$shucked_path")
set -l shucked_buffer (string join ' ' -- (string escape -- $shucked_words))
set -l shucked_count 0
for shucked_candidate in (complete --do-complete "$shucked_buffer")
    set shucked_count (math "$shucked_count + 1")
    test "$shucked_count" -le 2000; or break
    set -l shucked_fields (string split -m 1 \t -- "$shucked_candidate")
    set -l shucked_text (string unescape -- "$shucked_fields[1]")
    printf 'C\000%s\000%s\000\0000\000' "$shucked_text" "$shucked_fields[2]"
end
printf 'E\000'

end
