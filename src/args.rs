//! A small argument parser for this program's fixed command set.
//!
//! Hand-written rather than pulled from a crate: the grammar is a verb, an
//! optional subverb, some positional values, and `--flag`/`--flag value`
//! options. Keeping it here means the CLI has no dependency that could
//! change its behaviour underneath the archive.

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    /// Verbs, in order: e.g. `["story", "add"]`.
    pub command: Vec<String>,
    /// Non-flag values that followed the verbs.
    pub positional: Vec<String>,
    /// `--name value` pairs. A repeated flag keeps every value.
    options: BTreeMap<String, Vec<String>>,
    /// `--name` with no value.
    flags: Vec<String>,
}

/// Options that take no value, so `--voice add` does not swallow `add`.
const BOOLEAN_FLAGS: [&str; 8] = [
    "help",
    "voice",
    "suggest-followups",
    "date-from-exif",
    "dry-run",
    "apply",
    "plaintext-escrow",
    "version",
];

impl Args {
    /// Parse `argv` (without the program name).
    ///
    /// `group_commands` names the verbs that take a subverb (`story add`,
    /// `interview start`). Everything else takes exactly one verb, so
    /// `legacy init ./my-archive` reads the path as a value rather than
    /// mistaking it for a subcommand.
    pub fn parse_with(argv: &[String], group_commands: &[&str]) -> Args {
        let takes_subverb = argv
            .first()
            .is_some_and(|first| group_commands.contains(&first.as_str()));
        Args::parse(argv, if takes_subverb { 2 } else { 1 })
    }

    /// Parse with an explicit cap on how many leading words are verbs.
    pub fn parse(argv: &[String], max_verbs: usize) -> Args {
        let mut command = Vec::new();
        let mut positional = Vec::new();
        let mut options: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut flags = Vec::new();

        let mut index = 0;
        while index < argv.len() {
            let Some(token) = argv.get(index) else { break };

            if let Some(name) = token.strip_prefix("--") {
                // `--name=value` and `--name value` are both accepted.
                if let Some((name, value)) = name.split_once('=') {
                    options
                        .entry(name.to_owned())
                        .or_default()
                        .push(value.to_owned());
                    index += 1;
                    continue;
                }
                if BOOLEAN_FLAGS.contains(&name) {
                    flags.push(name.to_owned());
                    index += 1;
                    continue;
                }
                match argv.get(index + 1) {
                    Some(value) if !value.starts_with('-') => {
                        options
                            .entry(name.to_owned())
                            .or_default()
                            .push(value.clone());
                        index += 2;
                    }
                    Some(_) | None => {
                        flags.push(name.to_owned());
                        index += 1;
                    }
                }
                continue;
            }

            if let Some(short) = token.strip_prefix('-') {
                // Only `-a` (archive) and `-h` (help) exist.
                match short {
                    "h" => flags.push("help".to_owned()),
                    "a" => {
                        if let Some(value) = argv.get(index + 1) {
                            options
                                .entry("archive".to_owned())
                                .or_default()
                                .push(value.clone());
                            index += 1;
                        }
                    }
                    _unknown => flags.push(short.to_owned()),
                }
                index += 1;
                continue;
            }

            if command.len() < max_verbs && positional.is_empty() {
                command.push(token.clone());
            } else {
                positional.push(token.clone());
            }
            index += 1;
        }

        Args {
            command,
            positional,
            options,
            flags,
        }
    }

    pub fn verb(&self, position: usize) -> Option<&str> {
        self.command.get(position).map(String::as_str)
    }

    pub fn positional(&self, position: usize) -> Option<&str> {
        self.positional.get(position).map(String::as_str)
    }

    /// The last value given for `name`, if any.
    pub fn option(&self, name: &str) -> Option<&str> {
        self.options
            .get(name)
            .and_then(|values| values.last())
            .map(String::as_str)
    }

    /// Every value given for `name`, in order. Used by `--share`.
    pub fn option_all(&self, name: &str) -> Vec<String> {
        self.options.get(name).cloned().unwrap_or_default()
    }

    pub fn has_flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    /// A comma-separated option split into trimmed, non-empty parts.
    pub fn option_csv(&self, name: &str) -> Vec<String> {
        self.option(name)
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn option_u64(&self, name: &str, default: u64) -> u64 {
        self.option(name)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(default)
    }
}

#[cfg(test)]
mod tests {
    use super::Args;

    fn parse(tokens: &[&str], max_verbs: usize) -> Args {
        let owned: Vec<String> = tokens.iter().map(|token| (*token).to_owned()).collect();
        Args::parse(&owned, max_verbs)
    }

    #[test]
    fn separates_verbs_from_options() {
        let args = parse(&["story", "add", "--title", "Starting university"], 2);
        assert_eq!(args.verb(0), Some("story"));
        assert_eq!(args.verb(1), Some("add"));
        assert_eq!(args.option("title"), Some("Starting university"));
    }

    #[test]
    fn accepts_equals_form() {
        let args = parse(&["story", "add", "--title=Hello", "--date=1994"], 2);
        assert_eq!(args.option("title"), Some("Hello"));
        assert_eq!(args.option("date"), Some("1994"));
    }

    #[test]
    fn boolean_flags_do_not_swallow_the_next_word() {
        let args = parse(&["interview", "start", "--voice", "childhood"], 2);
        assert!(args.has_flag("voice"));
        assert_eq!(args.positional(0), Some("childhood"));
    }

    #[test]
    fn a_repeated_option_keeps_every_value() {
        let args = parse(&["unseal", "--share", "one", "--share", "two"], 1);
        assert_eq!(args.option_all("share"), vec!["one", "two"]);
        // A single lookup still returns the most recent.
        assert_eq!(args.option("share"), Some("two"));
    }

    #[test]
    fn short_archive_flag_takes_a_value() {
        let args = parse(&["verify", "-a", "./my-archive"], 1);
        assert_eq!(args.option("archive"), Some("./my-archive"));
    }

    #[test]
    fn positional_values_follow_the_verbs() {
        let args = parse(&["init", "./my-archive", "--subject", "Jane"], 1);
        assert_eq!(args.verb(0), Some("init"));
        assert_eq!(args.positional(0), Some("./my-archive"));
        assert_eq!(args.option("subject"), Some("Jane"));
    }

    #[test]
    fn splits_comma_separated_values() {
        let args = parse(&["story", "add", "--tags", "education, leaving-home ,"], 2);
        assert_eq!(args.option_csv("tags"), vec!["education", "leaving-home"]);
        assert!(args.option_csv("people").is_empty());
    }

    #[test]
    fn parses_numeric_options_with_a_default() {
        let args = parse(&["init", "x", "--threshold", "2"], 1);
        assert_eq!(args.option_u64("threshold", 3), 2);
        assert_eq!(args.option_u64("shares", 5), 5);
        // A non-numeric value falls back rather than panicking.
        let bad = parse(&["init", "x", "--threshold", "many"], 1);
        assert_eq!(bad.option_u64("threshold", 3), 3);
    }

    #[test]
    fn a_value_option_with_no_value_becomes_a_flag() {
        let args = parse(&["story", "list", "--year"], 2);
        assert!(args.has_flag("year"));
        assert_eq!(args.option("year"), None);
    }

    #[test]
    fn a_one_verb_command_keeps_its_path_as_a_value() {
        // `init` has no subcommand, so the path must stay positional
        // rather than being swallowed as a second verb.
        let groups = ["story", "interview"];
        let owned = argv(&["init", "./my-archive", "--subject", "Jane"]);
        let args = Args::parse_with(&owned, &groups);
        assert_eq!(args.command, vec!["init"]);
        assert_eq!(args.positional(0), Some("./my-archive"));
    }

    #[test]
    fn a_group_command_takes_its_subverb() {
        let groups = ["story", "interview"];
        let owned = argv(&["interview", "start", "childhood"]);
        let args = Args::parse_with(&owned, &groups);
        assert_eq!(args.command, vec!["interview", "start"]);
        assert_eq!(args.positional(0), Some("childhood"));
    }

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(|token| (*token).to_owned()).collect()
    }

    #[test]
    fn recognizes_help_in_both_spellings() {
        assert!(parse(&["--help"], 1).has_flag("help"));
        assert!(parse(&["-h"], 1).has_flag("help"));
    }

    #[test]
    fn a_negative_looking_value_is_still_a_flag_boundary() {
        // `--body` followed by another flag must not consume it.
        let args = parse(&["story", "add", "--body", "--title", "T"], 2);
        assert!(args.has_flag("body"));
        assert_eq!(args.option("title"), Some("T"));
    }
}
