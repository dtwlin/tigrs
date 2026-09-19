
use builtin;
use str;

set edit:completion:arg-completer[tigrs] = {|@words|
    fn spaces {|n|
        builtin:repeat $n ' ' | str:join ''
    }
    fn cand {|text desc|
        edit:complex-candidate $text &display=$text' '(spaces (- 14 (wcswidth $text)))$desc
    }
    var command = 'tigrs'
    for word $words[1..-1] {
        if (str:has-prefix $word '-') {
            break
        }
        set command = $command';'$word
    }
    var completions = [
        &'tigrs'= {
            cand -C 'Run as if tigrs was started in `<directory>` instead of the current working directory'
            cand --directory 'Run as if tigrs was started in `<directory>` instead of the current working directory'
            cand -c 'Optional configuration file path (defaults to ~/.config/tigrs/config.toml)'
            cand --config 'Optional configuration file path (defaults to ~/.config/tigrs/config.toml)'
            cand -s 'Target shell for `completions` subcommand (bash, zsh, fish, elvish, powershell)'
            cand --shell 'Target shell for `completions` subcommand (bash, zsh, fish, elvish, powershell)'
            cand -v 'Print version'
            cand --version 'Print version'
            cand --update-mode 'Launch with repository update mode enabled (disables default read-only protection)'
            cand --debug-frame-stats 'Hidden debug flag to log per-frame rendering metrics (bytes, duration µs, dirty rows)'
            cand -h 'Print help (see more with ''--help'')'
            cand --help 'Print help (see more with ''--help'')'
        }
    ]
    $completions[$command]
}
