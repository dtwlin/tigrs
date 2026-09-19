// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (C) 2026 David Lin <dtwlin@gmail.com>

//! CLI argument definitions and parsing.

use clap::Parser;
use std::path::PathBuf;

const SUBCOMMANDS_HELP: &str = "\
Subcommands:
  main [rev_args...] [-- path...]  Browse commit history with DAG graph and split diff pane (default)
  log [rev_args...] [-- path...]   Browse commit log with full messages and diffstats
  status                           Inspect working tree status; stage, unstage, or discard changes
  show [rev]                       Display commit header, diffstat, and patch (defaults to HEAD)
  blame [rev] <path>               Annotate file lines with originating commit, author, and date
  tree [rev] [path]                Browse repository directory tree at a revision (defaults to HEAD)
  blob [rev] <path>                View syntax-highlighted file contents at a revision
  refs                             Browse branches, remote-tracking refs, and tags
  stash                            Browse and inspect Git stash entries
  reflog [ref]                     Browse reference log entries (defaults to HEAD)
  grep [pattern]                   Search tracked files in the working tree for a pattern
  completions <shell>              Generate shell completions (bash, zsh, fish, elvish, powershell)
  man                              Generate the UNIX manual page (tigrs.1) in roff format to stdout
  version                          Print version information

Piped Mode:
  git diff | tigrs                 Read piped diff or log output from stdin in the interactive pager";

/// Fast, memory-safe text-mode interface for Git written in pure Rust.
#[derive(Parser, Debug)]
#[command(
    name = "tigrs",
    version = tigrs_core::APP_VERSION,
    disable_version_flag = true,
    about = "Fast, memory-safe text-mode interface for Git written in pure Rust",
    after_help = SUBCOMMANDS_HELP
)]
pub struct CliArgs {
    /// Print version information (`tigrs 0.1`).
    #[arg(
        short = 'v',
        short_alias = 'V',
        long = "version",
        action = clap::ArgAction::Version,
        help = "Print version"
    )]
    pub version: (),

    /// Subcommand or view mode (see Subcommands below)
    #[arg(default_value = "log", hide_default_value = true)]
    pub subcommand: String,

    /// Run as if tigrs was started in `<directory>` instead of the current working directory.
    #[arg(short = 'C', long = "directory")]
    pub directory: Option<PathBuf>,

    /// Optional configuration file path (defaults to ~/.config/tigrs/config.toml).
    #[arg(short = 'c', long = "config")]
    pub config: Option<PathBuf>,

    /// Git revision, branch, or commit range arguments.
    #[arg(trailing_var_arg = true)]
    pub rev_args: Vec<String>,

    /// Target shell for `completions` subcommand (bash, zsh, fish, elvish, powershell).
    #[arg(short = 's', long = "shell")]
    pub shell: Option<String>,

    /// Launch in read-only mode (disables staging, reverting, editor, and shell mutations).
    #[arg(
        long = "read-only",
        alias = "ro",
        help = "Launch in read-only mode (disables staging, reverting, editor, and shell mutations)"
    )]
    pub read_only: bool,

    /// Launch with repository update mode enabled (default; overrides `general.read_only = true` in `config.toml`).
    #[arg(
        long = "update-mode",
        help = "Launch with repository update mode enabled (default; overrides general.read_only = true in config.toml)"
    )]
    pub update_mode: bool,

    /// Hidden debug flag to log per-frame rendering metrics (bytes, duration µs, dirty rows).
    #[arg(long = "debug-frame-stats", hide = true)]
    pub debug_frame_stats: bool,
}

impl CliArgs {
    /// Returns `true` when the `subcommand` positional argument was explicitly provided on the
    /// command line rather than filled by `default_value = "log"`.
    #[must_use]
    pub fn is_explicit_subcommand_from<I, T>(itr: I) -> bool
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        use clap::{CommandFactory, parser::ValueSource};
        Self::command()
            .try_get_matches_from(itr)
            .ok()
            .and_then(|m| m.value_source("subcommand"))
            == Some(ValueSource::CommandLine)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_cli_args() {
        let args = CliArgs::try_parse_from(["tigrs"]).expect("default parse");
        assert_eq!(args.subcommand, "log");
        assert_eq!(args.directory, None);
        assert_eq!(args.config, None);
        assert!(args.rev_args.is_empty());
        assert_eq!(args.shell, None);
    }

    #[test]
    fn test_subcommands_and_views() {
        let cases = [
            ("status", &[][..]),
            ("blame", &["src/main.rs"][..]),
            ("tree", &["HEAD:src"][..]),
            ("blob", &["HEAD:README.md"][..]),
            ("show", &["HEAD~1"][..]),
            ("refs", &[][..]),
            ("stash", &[][..]),
            ("reflog", &[][..]),
            ("grep", &["pattern"][..]),
            ("log", &["origin/main..HEAD"][..]),
            ("main", &[][..]),
            ("completions", &["zsh"][..]),
            ("man", &[][..]),
        ];

        for (subcmd, revs) in cases {
            let mut argv = vec!["tigrs", subcmd];
            argv.extend_from_slice(revs);
            let parsed = CliArgs::try_parse_from(argv).expect("parse subcommand");
            assert_eq!(parsed.subcommand, subcmd);
            assert_eq!(parsed.rev_args, revs);
        }
    }

    #[test]
    fn test_flags_and_options() {
        let args = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            "/path/to/repo",
            "-c",
            "/path/to/config.toml",
            "status",
        ])
        .expect("flags parse");

        assert_eq!(args.directory, Some(PathBuf::from("/path/to/repo")));
        assert_eq!(args.config, Some(PathBuf::from("/path/to/config.toml")));
        assert_eq!(args.subcommand, "status");
    }

    #[test]
    fn test_completions_shell_flag() {
        let args = CliArgs::try_parse_from(["tigrs", "completions", "-s", "fish"])
            .expect("completions flag");
        assert_eq!(args.subcommand, "completions");
        assert_eq!(args.shell.as_deref(), Some("fish"));
    }

    #[test]
    fn test_trailing_rev_arguments_and_pathspecs() {
        let args = CliArgs::try_parse_from([
            "tigrs",
            "log",
            "main..feature",
            "--",
            "crates/tigrs-ui",
            "crates/tigrs-git",
        ])
        .expect("trailing rev args");

        assert_eq!(args.subcommand, "log");
        assert_eq!(
            args.rev_args,
            vec!["main..feature", "--", "crates/tigrs-ui", "crates/tigrs-git"]
        );
    }

    #[test]
    fn test_unknown_flags_return_error() {
        assert!(CliArgs::try_parse_from(["tigrs", "--nonexistent-flag"]).is_err());
        assert!(CliArgs::try_parse_from(["tigrs", "-Z"]).is_err());
    }

    #[test]
    fn test_help_and_version_flags() {
        for flag in ["-h", "--help"] {
            let help = CliArgs::try_parse_from(["tigrs", flag]);
            assert!(help.is_err());
            let err_str = help.unwrap_err().to_string();
            assert!(err_str.contains("Usage:") && err_str.contains("Options:"));
            assert!(err_str.contains("Subcommands:"));
            for expected in [
                "main [rev_args...]",
                "log [rev_args...]",
                "status",
                "show [rev]",
                "blame [rev] <path>",
                "tree [rev] [path]",
                "blob [rev] <path>",
                "refs",
                "stash",
                "reflog [ref]",
                "grep [pattern]",
                "completions <shell>",
                "man",
                "version",
            ] {
                assert!(
                    err_str.contains(expected),
                    "Missing '{expected}' in {flag} output:\n{err_str}"
                );
            }
        }

        let version = CliArgs::try_parse_from(["tigrs", "--version"]);
        assert!(version.is_err());
        let ver_str = version.unwrap_err().to_string();
        assert!(ver_str.contains("tigrs"));
    }

    #[test]
    fn test_combined_directory_and_subcommand_arguments() {
        let args = CliArgs::try_parse_from([
            "tigrs",
            "-C",
            "/tmp/custom_repo",
            "blame",
            "src/lib.rs",
            "HEAD~2",
        ])
        .expect("parse combined args");

        assert_eq!(args.directory, Some(PathBuf::from("/tmp/custom_repo")));
        assert_eq!(args.subcommand, "blame");
        assert_eq!(args.rev_args, vec!["src/lib.rs", "HEAD~2"]);
        assert!(!args.update_mode);
    }

    #[test]
    fn test_read_only_and_update_mode_flags() {
        let default_args = CliArgs::try_parse_from(["tigrs", "status"]).expect("default status");
        assert!(!default_args.read_only);
        assert!(!default_args.update_mode);

        let ro_args =
            CliArgs::try_parse_from(["tigrs", "--read-only", "status"]).expect("read-only");
        assert!(ro_args.read_only);
        assert!(!ro_args.update_mode);
        assert_eq!(ro_args.subcommand, "status");

        let update_args =
            CliArgs::try_parse_from(["tigrs", "--update-mode", "status"]).expect("update-mode");
        assert!(update_args.update_mode);
        assert!(!update_args.read_only);
        assert_eq!(update_args.subcommand, "status");
    }

    #[test]
    fn test_is_explicit_subcommand_from() {
        assert!(!CliArgs::is_explicit_subcommand_from(["tigrs"]));
        assert!(!CliArgs::is_explicit_subcommand_from([
            "tigrs", "-C", "log"
        ]));
        assert!(CliArgs::is_explicit_subcommand_from([
            "tigrs", "-C", "log", "log"
        ]));
        assert!(CliArgs::is_explicit_subcommand_from(["tigrs", "log"]));
    }
}
