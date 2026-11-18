//! Read a game `stringtable.csv`: the language columns and the text of each key.

use std::collections::HashMap;
use std::fmt;

/// A malformed table; `record` counts from the header, which is record 1.
#[derive(Debug, Clone, PartialEq)]
pub struct StringtableError {
    pub record: usize,
    pub message: String,
}

impl fmt::Display for StringtableError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "record {}: {}", self.record, self.message)
    }
}

impl std::error::Error for StringtableError {}

/// The text of each key in each language.
#[derive(Debug, Clone, PartialEq)]
pub struct Stringtable {
    languages: Vec<String>,
    rows: HashMap<String, Vec<String>>,
}

/// Read a table whose header names the languages after the key column.
pub fn parse_stringtable(csv: &str) -> Result<Stringtable, StringtableError> {
    let csv = csv.strip_prefix('\u{feff}').unwrap_or(csv);
    let mut records = split_records(csv)?.into_iter();
    let header = records.next().ok_or_else(|| StringtableError {
        record: 1,
        message: "no header row".to_owned(),
    })?;
    let languages: Vec<String> = split_fields(header).into_iter().skip(1).collect();
    let width = languages.len();
    let rows = records
        .map(|record| {
            let mut cells = split_fields(record).into_iter();
            let key = cells.next().unwrap_or_default();
            let mut row: Vec<String> = cells.collect();
            row.resize(width, String::new());
            (key, row)
        })
        .collect();
    Ok(Stringtable { languages, rows })
}

fn split_records(text: &str) -> Result<Vec<&str>, StringtableError> {
    let mut records = Vec::new();
    let mut quoted = false;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '\n' if !quoted => {
                push_record(&mut records, &text[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    if quoted {
        return Err(StringtableError {
            record: records.len() + 1,
            message: "unterminated quote".to_owned(),
        });
    }
    push_record(&mut records, &text[start..]);
    Ok(records)
}

fn push_record<'a>(records: &mut Vec<&'a str>, line: &'a str) {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if !line.is_empty() {
        records.push(line);
    }
}

fn split_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(c),
        }
    }
    fields.push(field);
    fields
}

impl Stringtable {
    /// Language names in header order.
    pub fn languages(&self) -> &[String] {
        &self.languages
    }

    /// The text of `key` (with or without `#`) in `language`; an empty cell falls back to english, then original.
    pub fn lookup(&self, key: &str, language: &str) -> Option<&str> {
        let row = self.rows.get(key.strip_prefix('#').unwrap_or(key))?;
        [language, "english", "original"]
            .into_iter()
            .filter_map(|name| self.column(name))
            .filter_map(|column| row.get(column))
            .find(|cell| !cell.is_empty())
            .map(String::as_str)
    }

    /// Replace the rows of `later` that share a key; language columns are matched by name, so a
    /// language `later` lacks is blank for those keys.
    pub fn merge(&mut self, later: Stringtable) {
        let columns: Vec<usize> = later
            .languages
            .iter()
            .map(|name| self.column_or_add(name))
            .collect();
        let width = self.languages.len();
        for row in self.rows.values_mut() {
            row.resize(width, String::new());
        }
        for (key, cells) in later.rows {
            let mut row = vec![String::new(); width];
            for (cell, &column) in cells.into_iter().zip(&columns) {
                row[column] = cell;
            }
            self.rows.insert(key, row);
        }
    }

    /// Number of keys.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the table has no keys.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    fn column(&self, language: &str) -> Option<usize> {
        self.languages
            .iter()
            .position(|name| name.eq_ignore_ascii_case(language))
    }

    fn column_or_add(&mut self, name: &str) -> usize {
        match self.column(name) {
            Some(column) => column,
            None => {
                self.languages.push(name.to_owned());
                self.languages.len() - 1
            }
        }
    }
}
