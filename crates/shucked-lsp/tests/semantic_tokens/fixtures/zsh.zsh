#!/usr/bin/env zsh
# zsh-specific surface: flags, modifiers, globs, blocks.
setopt extended_glob null_glob
autoload -Uz compinit
alias -g G='| grep'
unfunction old_helper

function helper {
  local -a lines
  lines=(${(f)"$(cat file)"})
  print -l ${(s/:/)PATH} ${path[1]} ${~pattern} ${#lines} ${lines:h} $pipestatus[1]
}

() { echo $1 } arg

foreach file in *.txt(N) **/*.log(.)
do
  echo $file:t
done

repeat 3 do echo again; done
print -l *(.) ~/*.txt(N) $path[1]

{ helper } always { echo cleanup }

if [[ -d $HOME ]] { echo home }
typeset -f helper
