// Repository-authored completion metadata, also available without native providers.

pub(super) struct Spec {
    pub words: &'static str,
    pub flags: &'static str,
    pub values: &'static str,
    pub subcommands: &'static str,
}

pub(super) const SPECS: &[Spec] = &[
    Spec {
        words: "brew",
        flags: "--version --help --verbose --debug",
        values: "",
        subcommands: "install uninstall reinstall upgrade update list info search outdated cleanup doctor config tap untap pin unpin link unlink services autoremove fetch",
    },
    Spec {
        words: "brew install",
        flags: "--formula --cask --dry-run --force --verbose --debug --quiet --build-from-source --HEAD --ignore-dependencies --only-dependencies",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "brew uninstall",
        flags: "--formula --cask --force --ignore-dependencies --zap",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "brew upgrade",
        flags: "--formula --cask --dry-run --force --greedy --verbose",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "pacman",
        flags: "-S -R -Q -U -F -D -T --sync --remove --query --upgrade --files --database --deptest --help --version --verbose --needed --noconfirm --downloadonly --refresh --sysupgrade --search --info --list --quiet --recursive --nosave --root --dbpath --config --cachedir --sysroot",
        values: "--root --dbpath --config --cachedir --sysroot -r -b",
        subcommands: "",
    },
    Spec {
        words: "git",
        flags: "--version --help -C -c --git-dir --work-tree --no-pager --bare",
        values: "-C -c --git-dir --work-tree",
        subcommands: "add bisect branch checkout cherry-pick clean clone commit diff fetch grep init log merge mv pull push rebase remote reset restore revert rm show stash status switch tag worktree",
    },
    Spec {
        words: "git add",
        flags: "--all --dry-run --force --interactive --patch --update --verbose -A -n -f -p -u -v",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "git commit",
        flags: "--all --amend --message --no-edit --signoff --verbose -a -m -s -v",
        values: "--message -m",
        subcommands: "",
    },
    Spec {
        words: "git diff",
        flags: "--cached --staged --stat --name-only --name-status --check --color --no-color --word-diff",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "git log",
        flags: "--oneline --graph --all --decorate --stat --patch --max-count --author --grep -n -p",
        values: "--max-count --author --grep -n",
        subcommands: "",
    },
    Spec {
        words: "git status",
        flags: "--short --branch --porcelain --untracked-files -s -b",
        values: "--untracked-files",
        subcommands: "",
    },
    Spec {
        words: "git checkout",
        flags: "--force --detach --track -b -B -f",
        values: "-b -B",
        subcommands: "",
    },
    Spec {
        words: "git switch",
        flags: "--create --force-create --detach --track -c -C",
        values: "--create --force-create -c -C",
        subcommands: "",
    },
    Spec {
        words: "git branch",
        flags: "--all --delete --force --list --move --remotes --verbose -a -d -D -m -r -v",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "git fetch",
        flags: "--all --prune --tags --depth --dry-run",
        values: "--depth",
        subcommands: "",
    },
    Spec {
        words: "git pull",
        flags: "--rebase --no-rebase --ff-only --no-ff --autostash",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "git push",
        flags: "--all --delete --dry-run --force-with-lease --set-upstream --tags -u",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "git remote",
        flags: "--verbose -v",
        values: "",
        subcommands: "add get-url prune remove rename set-head set-url show update",
    },
    Spec {
        words: "git stash",
        flags: "--include-untracked --all --patch -u -a -p",
        values: "",
        subcommands: "apply branch clear drop list pop push show",
    },
    Spec {
        words: "git worktree",
        flags: "",
        values: "",
        subcommands: "add list lock move prune remove repair unlock",
    },
    Spec {
        words: "curl",
        flags: "--data --data-raw --fail --header --head --location --output --request --show-error --silent --url --user-agent -d -f -H -I -L -o -X -S -s",
        values: "--data --data-raw --header --output --request --url --user-agent -d -H -o -X",
        subcommands: "",
    },
    Spec {
        words: "ssh",
        flags: "-4 -6 -A -a -C -F -i -J -L -N -o -p -R -T -t -v",
        values: "-F -i -J -L -o -p -R",
        subcommands: "",
    },
    Spec {
        words: "grep",
        flags: "-E -F -c -e -f -i -l -n -q -s -v -x",
        values: "-e -f",
        subcommands: "",
    },
    Spec {
        words: "sed",
        flags: "-e -f -n",
        values: "-e -f",
        subcommands: "",
    },
    Spec {
        words: "awk",
        flags: "-F -f -v",
        values: "-F -f -v",
        subcommands: "",
    },
    Spec {
        words: "find",
        flags: "-H -L -name -path -type -user -group -size -mtime -print -exec -prune",
        values: "-name -path -type -user -group -size -mtime -exec",
        subcommands: "",
    },
    Spec {
        words: "ls",
        flags: "-a -A -d -F -h -i -l -n -r -R -S -t -u",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "cp",
        flags: "-f -i -p -R",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "mv",
        flags: "-f -i",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "rm",
        flags: "-f -i -r -R",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "mkdir",
        flags: "-m -p",
        values: "-m",
        subcommands: "",
    },
    Spec {
        words: "tar",
        flags: "-c -t -x -f -v -z -j -C",
        values: "-f -C",
        subcommands: "",
    },
    Spec {
        words: "cd",
        flags: "-L -P",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "read",
        flags: "-r",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "printf",
        flags: "",
        values: "",
        subcommands: "",
    },
    Spec {
        words: "docker",
        flags: "--config --context --debug --host --version -D -H -v",
        values: "--config --context --host -H",
        subcommands: "build compose container context exec image images info inspect logs network ps pull push rm rmi run start stop system version volume",
    },
    Spec {
        words: "docker compose",
        flags: "--file --project-name --profile -f -p",
        values: "--file --project-name --profile -f -p",
        subcommands: "build config down exec images logs ls ps pull push restart rm run start stop up version",
    },
    Spec {
        words: "docker run",
        flags: "--detach --env --env-file --interactive --name --network --publish --rm --tty --volume --workdir -d -e -i -p -t -v -w",
        values: "--env --env-file --name --network --publish --volume --workdir -e -p -v -w",
        subcommands: "",
    },
    Spec {
        words: "kubectl",
        flags: "--context --kubeconfig --namespace -n",
        values: "--context --kubeconfig --namespace -n",
        subcommands: "apply attach auth cluster-info config cp create delete describe diff edit exec explain get label logs patch port-forward replace rollout scale set top version wait",
    },
    Spec {
        words: "kubectl get",
        flags: "--all-namespaces --output --selector --watch -A -o -l -w",
        values: "--output --selector -o -l",
        subcommands: "pods services deployments namespaces nodes secrets configmaps statefulsets daemonsets jobs cronjobs ingress",
    },
];

pub(super) struct Arguments {
    pub flags: Vec<&'static str>,
    pub subcommands: Vec<&'static str>,
    pub expecting_value: bool,
    pub after_separator: bool,
}

pub(super) fn arguments(words: &[String]) -> Arguments {
    let mut result = Arguments {
        flags: Vec::new(),
        subcommands: Vec::new(),
        expecting_value: false,
        after_separator: false,
    };
    result.after_separator = words.iter().skip(1).any(|word| word == "--");
    let Some(command) = words.first() else {
        return result;
    };
    let command = command.rsplit('/').next().unwrap_or(command);
    let Some(mut spec) = SPECS.iter().find(|spec| spec.words == command) else {
        return result;
    };
    result.after_separator = false;
    let mut matched = command.to_owned();
    let mut positional = false;
    result.flags.extend(spec.flags.split_whitespace());
    for word in &words[1..] {
        if result.expecting_value {
            result.expecting_value = false;
            continue;
        }
        if word == "--" {
            result.after_separator = true;
            break;
        }
        if word.starts_with('-') {
            result.expecting_value = spec.values.split_whitespace().any(|value| value == word);
            continue;
        }
        if !positional && spec.subcommands.split_whitespace().any(|name| name == word) {
            matched.push(' ');
            matched.push_str(word);
            if let Some(child) = SPECS.iter().find(|candidate| candidate.words == matched) {
                spec = child;
                result.flags.clear();
                result.flags.extend(spec.flags.split_whitespace());
            } else {
                // Do not offer parent options as options for an unknown subcommand.
                result.flags.clear();
                positional = true;
            }
        } else {
            positional = true;
        }
    }
    if !positional && !result.expecting_value && !result.after_separator {
        result
            .subcommands
            .extend(spec.subcommands.split_whitespace());
    }
    if command == "pacman" {
        let operation = words.iter().skip(1).find_map(|word| match word.as_str() {
            "--sync" => Some('S'),
            "--remove" => Some('R'),
            "--query" => Some('Q'),
            _ if word.starts_with('-') && !word.starts_with("--") => {
                word.chars().find(|ch| "SRQ".contains(*ch))
            }
            _ => None,
        });
        if let Some(operation) = operation {
            result.flags = "--help --verbose --root --dbpath --config --sysroot"
                .split_whitespace()
                .collect();
            result.flags.extend(match operation {
                'S' => "-s -i -l -y -u -w --search --info --list --refresh --sysupgrade --downloadonly --needed --asdeps --asexplicit --ignore --ignoregroup --overwrite",
                'R' => "-s -n -c -u --recursive --nosave --cascade --unneeded --print",
                _ => "-s -i -l -o -u -m -e -d --search --info --list --owns --upgrades --foreign --explicit --deps --quiet",
            }.split_whitespace());
        }
    }
    result.flags.retain(|flag| {
        !words
            .iter()
            .any(|word| word == flag || word.starts_with(&format!("{flag}=")))
    });
    result
}

pub(super) fn description(flag: &str) -> &'static str {
    match flag {
        "--formula" => "Use Homebrew formulae",
        "--cask" => "Use Homebrew casks",
        "--dry-run" => "Preview the operation",
        "--needed" => "Skip packages that are already current",
        "--sync" => "Use the package repository database",
        "--remove" => "Remove installed packages",
        "--query" => "Inspect installed packages",
        "--downloadonly" => "Download packages without installing",
        "--refresh" => "Refresh package repository metadata",
        "--sysupgrade" => "Upgrade installed packages",
        "--config" => "Choose a configuration file",
        "--dbpath" => "Choose a package database directory",
        "--help" => "Show command help",
        "--version" => "Show the installed version",
        _ => "Command option",
    }
}
