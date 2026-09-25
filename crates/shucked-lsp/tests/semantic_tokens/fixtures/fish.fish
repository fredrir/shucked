#!/usr/bin/env fish
# Fish keywords, strings, variables, numbers and options.
set -l greeting "hello $USER" 'literal $x'
set -q missing_var; or set -g missing_var 42

function greet --description "say hello" -a name
  if test -f $argv[1]; and not contains -- $name $argv
    echo "$greeting $name" | cat
  else if [ $status -eq 0 ]
    return 1
  else
    command ls --long -a
  end
end

for item in one two 3
  switch $item
    case one
      echo one
    case '*'
      echo (ls | head -n 1)
  end
end

while begin; not test -d /tmp; end
  echo "multi
line $HOME string" > /dev/null 2>&1
  break
end

greet world; missing_tool_fish
