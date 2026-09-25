#!/bin/bash
# Exercises every construct the semantic token walk covers.
set -euo pipefail

readonly LIMIT=3
export PREFIX="/opt/tool" # trailing comment
declare -rx FLAG=1 arr=([k]=v [2]=w)
typeset -f greet
unset LIMIT

greet() {
  local name=${1:-world} count=0
  echo "hello $name, ${name^}, ${#name}, ${name//o/0}, $(date +%s), $((count + 1))"
  echo 'single $quoted' $'ansi\n' "héllo wörld"
  return 0
}

function twice {
  greet "$@" && greet "$*" || exit 1
  echo "$? $$ $! $- $_ $0 $#" | cat
  printf '%s\n' "${arr[@]}" "${arr[1]}" "${!arr[@]}" "${!PRE@}" "${PREFIX:1:2}" "${FLAG@Q}" "${!ref}"
}

if [[ -f "$PREFIX/bin" && $name == a* || ! ( $name =~ ^r.*$ ) ]]; then
  ls -la --color=auto -- -notanoption
elif [ -z "$name" -a "$count" -eq 5 ]; then
  # a comment between branches
  sleep 0.5
else
  missing_tool --version
fi

for item in one two 3; do
  [[ $item -nt /tmp ]] || continue 2
done
for ((i = 0; i < LIMIT; i++)); do (( total += i * 2 )); break; done
while read -r line; do echo "$line"; done < input.txt
until false; do :; done
select choice in a b; do echo "$choice"; done
case $item in
  (one|t*) echo one ;;
  [ab]?) echo ab ;&
  @(x|y)) echo ext ;;
  *) ;;
esac

cat <<EOF >out.txt 2>&1
body $name ${name:-x} $(date) $((1 + 2)) literal
EOF
cat <<-'RAW' | sort
	no $expansion here
	RAW
cat <<< "here string $name" &> /dev/null
exec {fd}>log.txt
exec 3>&-
diff <(ls) >(cat) &
! grep -q pattern file
command -v ls; builtin echo hi; exec env FOO=1 ls; env -i ls
time -p sleep 1
coproc COP { cat; }
alias ll='ls -l'
unalias ll
git commit -m "message with $(git rev-parse --short HEAD)"
