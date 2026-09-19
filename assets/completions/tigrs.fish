complete -c tigrs -s C -l directory -d 'Run as if tigrs was started in `<directory>` instead of the current working directory' -r -F
complete -c tigrs -s c -l config -d 'Optional configuration file path (defaults to ~/.config/tigrs/config.toml)' -r -F
complete -c tigrs -s s -l shell -d 'Target shell for `completions` subcommand (bash, zsh, fish, elvish, powershell)' -r
complete -c tigrs -s v -l version -d 'Print version'
complete -c tigrs -l read-only -d 'Launch in read-only mode (disables staging, reverting, editor, and shell mutations)'
complete -c tigrs -l update-mode -d 'Launch with repository update mode enabled (default; overrides general.read_only = true in config.toml)'
complete -c tigrs -l debug-frame-stats -d 'Hidden debug flag to log per-frame rendering metrics (bytes, duration µs, dirty rows)'
complete -c tigrs -s h -l help -d 'Print help'
