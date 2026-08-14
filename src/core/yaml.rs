//! A deliberately small YAML layer for the archive's fixed schemas.
//!
//! Reading is done with a real YAML parser, so hand-edited files and files
//! written by other tools still load. Writing is done by the explicit
//! emitter below rather than a generic serializer, because the exact bytes
//! we put on disk are part of the archive's contract: field order is fixed
//! so files diff cleanly, and every scalar that could be misread as a
//! number, boolean, or date is quoted. An unquoted `1994-09-15` is a
//! timestamp to some YAML readers and a string to others; a 30-year archive
//! cannot afford that ambiguity.

use serde_yaml_ng::Value;

/// Scalars that a YAML reader could mistake for a non-string value.
const RESERVED_WORDS: [&str; 22] = [
    "true", "false", "yes", "no", "on", "off", "null", "nil", "none", "~", "y", "n", "True",
    "False", "Yes", "No", "On", "Off", "Null", "NULL", "None", "TRUE",
];

const LEADING_INDICATORS: [char; 15] = [
    '-', '?', ':', ',', '[', ']', '{', '}', '#', '&', '*', '!', '|', '>', '%',
];

fn looks_numeric(value: &str) -> bool {
    value.parse::<f64>().is_ok() || value.parse::<i64>().is_ok()
}

/// True when `value` reads as a date-like scalar (`1994`, `1994-09`,
/// `1994-09-15`, `1970s`) that a reader might coerce.
fn looks_datelike(value: &str) -> bool {
    let segments: Vec<&str> = value.split('-').collect();
    let numeric = segments
        .iter()
        .all(|segment| !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()));
    if numeric && segments.len() > 1 {
        return true;
    }
    value
        .strip_suffix('s')
        .is_some_and(|stem| !stem.is_empty() && stem.chars().all(|c| c.is_ascii_digit()))
}

fn needs_quoting(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    if value.trim() != value {
        return true;
    }
    if RESERVED_WORDS.contains(&value) {
        return true;
    }
    if looks_numeric(value) || looks_datelike(value) {
        return true;
    }
    if value.contains(": ") || value.contains(" #") || value.contains('\n') {
        return true;
    }
    if value.ends_with(':') {
        return true;
    }
    value
        .chars()
        .next()
        .is_some_and(|first| LEADING_INDICATORS.contains(&first) || first == '\'' || first == '"')
}

/// Render a string as a YAML scalar, single-quoting when necessary.
/// Single quotes need no escape handling beyond doubling an inner quote,
/// which keeps the output legible to a human reader.
pub fn scalar(value: &str) -> String {
    if needs_quoting(value) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        value.to_owned()
    }
}

/// A YAML document built one field at a time, in a fixed order.
#[derive(Default)]
pub struct Emitter {
    buffer: String,
}

impl Emitter {
    pub fn new() -> Emitter {
        Emitter {
            buffer: String::new(),
        }
    }

    pub fn string(&mut self, key: &str, value: &str) -> &mut Emitter {
        self.buffer.push_str(&format!("{key}: {}\n", scalar(value)));
        self
    }

    pub fn optional_string(&mut self, key: &str, value: Option<&str>) -> &mut Emitter {
        match value {
            Some(value) => self.string(key, value),
            None => self.null(key),
        }
    }

    pub fn null(&mut self, key: &str) -> &mut Emitter {
        self.buffer.push_str(&format!("{key}: null\n"));
        self
    }

    pub fn unsigned(&mut self, key: &str, value: u64) -> &mut Emitter {
        self.buffer.push_str(&format!("{key}: {value}\n"));
        self
    }

    pub fn optional_unsigned(&mut self, key: &str, value: Option<u64>) -> &mut Emitter {
        match value {
            Some(value) => self.unsigned(key, value),
            None => self.null(key),
        }
    }

    pub fn optional_float(&mut self, key: &str, value: Option<f64>) -> &mut Emitter {
        match value {
            Some(value) => {
                self.buffer.push_str(&format!("{key}: {value}\n"));
                self
            }
            None => self.null(key),
        }
    }

    pub fn boolean(&mut self, key: &str, value: bool) -> &mut Emitter {
        self.buffer.push_str(&format!("{key}: {value}\n"));
        self
    }

    /// A block sequence of scalars, or `[]` when empty.
    pub fn list(&mut self, key: &str, values: &[String]) -> &mut Emitter {
        if values.is_empty() {
            self.buffer.push_str(&format!("{key}: []\n"));
            return self;
        }
        self.buffer.push_str(&format!("{key}:\n"));
        for value in values {
            self.buffer.push_str(&format!("- {}\n", scalar(value)));
        }
        self
    }

    /// A nested mapping of string to string, or `{}` when empty.
    pub fn string_map(&mut self, key: &str, entries: &[(String, String)]) -> &mut Emitter {
        if entries.is_empty() {
            self.buffer.push_str(&format!("{key}: {{}}\n"));
            return self;
        }
        self.buffer.push_str(&format!("{key}:\n"));
        for (name, value) in entries {
            self.buffer
                .push_str(&format!("  {}: {}\n", scalar(name), scalar(value)));
        }
        self
    }

    /// Begin a nested mapping; subsequent `indented_*` calls belong to it.
    pub fn open_map(&mut self, key: &str) -> &mut Emitter {
        self.buffer.push_str(&format!("{key}:\n"));
        self
    }

    pub fn raw(&mut self, line: &str) -> &mut Emitter {
        self.buffer.push_str(line);
        self.buffer.push('\n');
        self
    }

    pub fn finish(&self) -> String {
        self.buffer.clone()
    }
}

/// Read helpers over a parsed document. These are tolerant: a missing or
/// wrongly typed field yields `None` or a default rather than an error, so
/// one bad line never makes a story file unreadable.
pub fn as_mapping(document: &Value, key: &str) -> Option<Value> {
    document.get(key).cloned()
}

pub fn get_string(document: &Value, key: &str) -> Option<String> {
    match document.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Number(value)) => Some(value.to_string()),
        Some(Value::Bool(value)) => Some(value.to_string()),
        Some(_) | None => None,
    }
}

pub fn get_u64(document: &Value, key: &str) -> Option<u64> {
    document.get(key).and_then(Value::as_u64)
}

pub fn get_f64(document: &Value, key: &str) -> Option<f64> {
    document.get(key).and_then(Value::as_f64)
}

pub fn get_bool(document: &Value, key: &str, default: bool) -> bool {
    document
        .get(key)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

pub fn get_list(document: &Value, key: &str) -> Vec<String> {
    match document.get(key) {
        Some(Value::Sequence(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(value) => Some(value.clone()),
                Value::Number(value) => Some(value.to_string()),
                _ => None,
            })
            .collect(),
        Some(_) | None => Vec::new(),
    }
}

pub fn get_string_map(document: &Value, key: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    if let Some(Value::Mapping(mapping)) = document.get(key) {
        for (name, value) in mapping {
            if let (Value::String(name), Value::String(value)) = (name, value) {
                entries.push((name.clone(), value.clone()));
            }
        }
    }
    entries
}

pub fn parse(text: &str) -> Result<Value, String> {
    serde_yaml_ng::from_str::<Value>(text).map_err(|error| format!("invalid YAML: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{parse, scalar, Emitter};

    #[test]
    fn quotes_ambiguous_scalars() {
        assert_eq!(scalar("1994-09-15"), "'1994-09-15'");
        assert_eq!(scalar("1994"), "'1994'");
        assert_eq!(scalar("1970s"), "'1970s'");
        assert_eq!(scalar("true"), "'true'");
        assert_eq!(scalar("null"), "'null'");
        assert_eq!(scalar(""), "''");
        assert_eq!(scalar("- leading dash"), "'- leading dash'");
    }

    #[test]
    fn leaves_ordinary_prose_unquoted() {
        assert_eq!(scalar("Starting university"), "Starting university");
        assert_eq!(scalar("family"), "family");
        assert_eq!(scalar("mary-oconnor"), "mary-oconnor");
    }

    #[test]
    fn doubles_inner_single_quotes() {
        assert_eq!(scalar("Dad's boat"), "Dad's boat");
        assert_eq!(scalar("'quoted'"), "'''quoted'''");
    }

    #[test]
    fn round_trips_through_a_real_parser() {
        let mut emitter = Emitter::new();
        emitter
            .string("id", "1994-09-15-started-university")
            .string("date", "1994-09-15")
            .list("tags", &["education".to_owned(), "leaving-home".to_owned()])
            .list("people", &[])
            .null("source");
        let document = parse(&emitter.finish()).expect("emitted YAML should parse");

        assert_eq!(
            super::get_string(&document, "date").as_deref(),
            Some("1994-09-15")
        );
        assert_eq!(super::get_list(&document, "tags").len(), 2);
        assert!(super::get_list(&document, "people").is_empty());
        assert_eq!(super::get_string(&document, "source"), None);
    }
}
