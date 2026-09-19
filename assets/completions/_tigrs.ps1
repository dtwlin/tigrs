
using namespace System.Management.Automation
using namespace System.Management.Automation.Language

Register-ArgumentCompleter -Native -CommandName 'tigrs' -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)

    $commandElements = $commandAst.CommandElements
    $command = @(
        'tigrs'
        for ($i = 1; $i -lt $commandElements.Count; $i++) {
            $element = $commandElements[$i]
            if ($element -isnot [StringConstantExpressionAst] -or
                $element.StringConstantType -ne [StringConstantType]::BareWord -or
                $element.Value.StartsWith('-') -or
                $element.Value -eq $wordToComplete) {
                break
        }
        $element.Value
    }) -join ';'

    $completions = @(switch ($command) {
        'tigrs' {
            [CompletionResult]::new('-C', '-C ', [CompletionResultType]::ParameterName, 'Run as if tigrs was started in `<directory>` instead of the current working directory')
            [CompletionResult]::new('--directory', '--directory', [CompletionResultType]::ParameterName, 'Run as if tigrs was started in `<directory>` instead of the current working directory')
            [CompletionResult]::new('-c', '-c', [CompletionResultType]::ParameterName, 'Optional configuration file path (defaults to ~/.config/tigrs/config.toml)')
            [CompletionResult]::new('--config', '--config', [CompletionResultType]::ParameterName, 'Optional configuration file path (defaults to ~/.config/tigrs/config.toml)')
            [CompletionResult]::new('-s', '-s', [CompletionResultType]::ParameterName, 'Target shell for `completions` subcommand (bash, zsh, fish, elvish, powershell)')
            [CompletionResult]::new('--shell', '--shell', [CompletionResultType]::ParameterName, 'Target shell for `completions` subcommand (bash, zsh, fish, elvish, powershell)')
            [CompletionResult]::new('-v', '-v', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('--version', '--version', [CompletionResultType]::ParameterName, 'Print version')
            [CompletionResult]::new('--update-mode', '--update-mode', [CompletionResultType]::ParameterName, 'Launch with repository update mode enabled (disables default read-only protection)')
            [CompletionResult]::new('--debug-frame-stats', '--debug-frame-stats', [CompletionResultType]::ParameterName, 'Hidden debug flag to log per-frame rendering metrics (bytes, duration µs, dirty rows)')
            [CompletionResult]::new('-h', '-h', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            [CompletionResult]::new('--help', '--help', [CompletionResultType]::ParameterName, 'Print help (see more with ''--help'')')
            break
        }
    })

    $completions.Where{ $_.CompletionText -like "$wordToComplete*" } |
        Sort-Object -Property ListItemText
}
