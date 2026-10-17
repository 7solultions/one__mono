//! Reading and building helpers over the AST.
//!
//! The AST is plain data (`Vec<MappingEntry>`), which keeps it easy to
//! serialise, but a consumer that reads a settings file wants `get`, `set` and
//! `pairs` rather than a hand-written loop. These helpers are additive: nothing
//! about the AST's shape changes, so parser users that match on it directly are
//! unaffected.

use crate::ast::{MappingEntry, SequenceItem, SyonFile, Value};

impl Value {
    /// The text of a scalar or a `|` block.
    ///
    /// A `|` block's final newline is the line ending of its last line, not
    /// part of the content, so it is left off here. The tree itself keeps the
    /// exact text the parser produced.
    pub fn as_scalar(&self) -> Option<&str> {
        match self {
            Value::Scalar(text) => Some(text),
            Value::LiteralBlock(text) => Some(text.strip_suffix('\n').unwrap_or(text)),
            _ => None,
        }
    }

    pub fn as_mapping(&self) -> Option<Mapping<'_>> {
        match self {
            Value::Mapping(entries) => Some(Mapping::new(entries)),
            _ => None,
        }
    }

    pub fn as_sequence(&self) -> Option<&[SequenceItem]> {
        match self {
            Value::Sequence(items) => Some(items),
            _ => None,
        }
    }

    /// A word for error messages ("expected a mapping, found a sequence").
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Scalar(_) | Value::LiteralBlock(_) => "scalar",
            Value::Mapping(_) => "mapping",
            Value::Sequence(_) => "sequence",
        }
    }

    pub fn scalar(text: impl Into<String>) -> Self {
        Value::Scalar(text.into())
    }

    pub fn mapping(entries: Vec<MappingEntry>) -> Self {
        Value::Mapping(entries)
    }

    pub fn sequence(values: impl IntoIterator<Item = Value>) -> Self {
        Value::Sequence(values.into_iter().map(SequenceItem::new).collect())
    }
}

impl MappingEntry {
    /// An entry built in code: no source line, no comments.
    pub fn new(key: impl Into<String>, value: Value) -> Self {
        MappingEntry {
            line: 0,
            key: key.into(),
            value,
            leading_comments: Vec::new(),
            trailing_comment: None,
        }
    }
}

impl SequenceItem {
    pub fn new(value: Value) -> Self {
        SequenceItem {
            line: 0,
            value,
            leading_comments: Vec::new(),
            trailing_comment: None,
        }
    }
}

impl SyonFile {
    /// The main (unfenced) document's body, the one a settings file has.
    pub fn root(&self) -> Option<&Value> {
        self.documents
            .iter()
            .find(|document| document.path.is_none() && document.format.is_none())
            .map(|document| &document.body)
    }
}

/// A read-only view of a mapping's entries.
///
/// A view type and not a trait on `[MappingEntry]`: a slice has an inherent
/// `get(index)` that always wins over a trait method of the same name, so
/// `entries.get("key")` would not compile. The view carries the names readers
/// of a mapping expect (`get`, `contains_key`, `keys`, `iter`, `len`).
#[derive(Debug, Clone, Copy)]
pub struct Mapping<'a> {
    entries: &'a [MappingEntry],
}

impl<'a> Mapping<'a> {
    pub fn new(entries: &'a [MappingEntry]) -> Self {
        Mapping { entries }
    }

    pub fn get(&self, key: &str) -> Option<&'a Value> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| &entry.value)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn keys(&self) -> impl Iterator<Item = &'a str> {
        self.entries.iter().map(|entry| entry.key.as_str())
    }

    /// Key and value pairs in the order the mapping holds them.
    pub fn iter(&self) -> impl Iterator<Item = (&'a str, &'a Value)> {
        self.entries
            .iter()
            .map(|entry| (entry.key.as_str(), &entry.value))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The underlying entries, with their lines and comments.
    pub fn entries(&self) -> &'a [MappingEntry] {
        self.entries
    }
}

/// Insertion into a mapping under construction.
pub trait EntriesMut {
    /// Replaces the value in place if the key exists, else appends, so
    /// re-writing a file does not reorder its keys.
    fn set(&mut self, key: impl Into<String>, value: Value) -> &mut Self;
}

impl EntriesMut for Vec<MappingEntry> {
    fn set(&mut self, key: impl Into<String>, value: Value) -> &mut Self {
        let key = key.into();
        match self.iter_mut().find(|entry| entry.key == key) {
            Some(existing) => existing.value = value,
            None => self.push(MappingEntry::new(key, value)),
        }
        self
    }
}

/// The values of a sequence, without the per-item line and comment wrapper.
pub trait SequenceItems {
    fn values(&self) -> impl Iterator<Item = &Value>;
}

impl SequenceItems for [SequenceItem] {
    fn values(&self) -> impl Iterator<Item = &Value> {
        self.iter().map(|item| &item.value)
    }
}

/// A mapping from key and value pairs, in the order given.
pub fn mapping_from<K: Into<String>>(
    pairs: impl IntoIterator<Item = (K, Value)>,
) -> Vec<MappingEntry> {
    pairs
        .into_iter()
        .map(|(key, value)| MappingEntry::new(key, value))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    #[test]
    fn scalar_of_a_literal_block_drops_the_final_newline() {
        let file = parse("d: |\n  a\n  b\n").unwrap();
        let entries = file.root().unwrap().as_mapping().unwrap();
        assert_eq!(entries.get("d").unwrap().as_scalar(), Some("a\nb"));
        // The tree keeps what the parser produced.
        assert_eq!(
            entries.get("d").unwrap(),
            &Value::LiteralBlock("a\nb\n".into())
        );
    }

    #[test]
    fn scalar_of_a_plain_value_is_untouched() {
        assert_eq!(Value::Scalar("x\n".into()).as_scalar(), Some("x\n"));
    }

    #[test]
    fn non_scalars_have_no_scalar() {
        assert_eq!(Value::mapping(Vec::new()).as_scalar(), None);
        assert_eq!(Value::sequence([]).as_scalar(), None);
    }

    #[test]
    fn set_replaces_in_place_and_appends_new_keys() {
        let mut entries: Vec<MappingEntry> = Vec::new();
        entries
            .set("a", Value::scalar("1"))
            .set("b", Value::scalar("2"));
        entries.set("a", Value::scalar("3"));
        let view = Mapping::new(&entries);
        assert_eq!(view.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(view.get("a").and_then(Value::as_scalar), Some("3"));
        assert!(view.contains_key("b"));
        assert!(!view.contains_key("c"));
        assert_eq!((view.len(), view.is_empty()), (2, false));
    }

    #[test]
    fn pairs_walk_in_held_order() {
        let entries = mapping_from([("x", Value::scalar("1")), ("y", Value::scalar("2"))]);
        let pairs: Vec<_> = Mapping::new(&entries)
            .iter()
            .map(|(k, v)| (k, v.as_scalar().unwrap()))
            .collect();
        assert_eq!(pairs, vec![("x", "1"), ("y", "2")]);
    }

    #[test]
    fn sequence_values_skip_the_item_wrapper() {
        let value = Value::sequence([Value::scalar("a"), Value::scalar("b")]);
        let texts: Vec<_> = value
            .as_sequence()
            .unwrap()
            .values()
            .map(|v| v.as_scalar().unwrap())
            .collect();
        assert_eq!(texts, vec!["a", "b"]);
    }

    #[test]
    fn kind_names_match_the_old_parser() {
        assert_eq!(Value::scalar("x").kind_name(), "scalar");
        assert_eq!(Value::mapping(Vec::new()).kind_name(), "mapping");
        assert_eq!(Value::sequence([]).kind_name(), "sequence");
    }

    #[test]
    fn root_is_the_unfenced_document() {
        let file = parse("a: 1\n").unwrap();
        assert!(file.root().unwrap().as_mapping().unwrap().contains_key("a"));
    }

    #[test]
    fn parse_root_reads_compact_lists_the_default_rejects() {
        let text = "people:\n  - id: p1\n    role: a\n  - id: p2\n    role: b\n";
        assert!(
            parse(text).is_err(),
            "the default dialect rejects the compact form"
        );
        let root = crate::parse_root(text).unwrap();
        let people = root.as_mapping().unwrap().get("people").unwrap();
        assert_eq!(people.as_sequence().unwrap().len(), 2);
    }

    #[test]
    fn parse_root_of_an_empty_input_is_an_empty_mapping() {
        assert_eq!(crate::parse_root("").unwrap(), Value::mapping(Vec::new()));
    }
}
