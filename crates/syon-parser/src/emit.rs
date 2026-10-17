//! Writing SYON back out.
//!
//! The inverse of [`crate::parse`], kept beside it so the block-only style and
//! the rules for what may be written inline are decided once, here, rather than
//! in every crate that persists a document.
//!
//! * **Block style only**, keys in the order the tree holds them, so
//!   re-writing an unchanged tree is byte-identical and edits make small diffs.
//! * **Lists of mappings are compact** (`- id: p1`, the rest aligned below),
//!   the form existing files are written in. Read them back with
//!   [`crate::parse_root`].
//! * **A `|` literal block** for any scalar that cannot be written inline
//!   without changing its meaning when read back.
//! * **Comments** the tree carries are written.
//!
//! Round-trip limits, inherent to the format: leading whitespace common to every
//! line of a block is dropped by the reader, and a scalar whose text ends in
//! newlines is read back with a single trailing newline.

use crate::ast::{MappingEntry, SequenceItem, Value};

/// One indentation level: the width of a `- ` marker, so a compact item's
/// continuation lines line up under its first key.
const INDENT: usize = 2;

/// Renders `value` as a document, ending in a newline (empty for an empty
/// collection).
pub fn emit(value: &Value) -> String {
    let mut output = String::new();
    write_value(value, 0, &mut output);
    output
}

fn padding(depth: usize) -> String {
    " ".repeat(depth * INDENT)
}

fn write_value(value: &Value, depth: usize, output: &mut String) {
    match value {
        Value::Mapping(entries) => write_mapping(entries, depth, output),
        Value::Sequence(items) => write_sequence(items, depth, output),
        Value::Scalar(text) | Value::LiteralBlock(text) => {
            // A bare scalar document: the lines of its text.
            for line in text.lines() {
                output.push_str(&padding(depth));
                output.push_str(line);
                output.push('\n');
            }
        }
    }
}

fn write_comments(comments: &[String], pad: &str, output: &mut String) {
    for comment in comments {
        output.push_str(pad);
        if comment.is_empty() {
            output.push('#');
        } else {
            output.push_str("# ");
            output.push_str(comment);
        }
        output.push('\n');
    }
}

fn write_trailing(comment: &Option<String>, output: &mut String) {
    if let Some(text) = comment {
        if text.is_empty() {
            output.push_str(" #");
        } else {
            output.push_str(" # ");
            output.push_str(text);
        }
    }
}

fn write_mapping(entries: &[MappingEntry], depth: usize, output: &mut String) {
    let pad = padding(depth);
    for entry in entries {
        write_comments(&entry.leading_comments, &pad, output);
        output.push_str(&pad);
        output.push_str(&entry.key);
        output.push(':');
        match &entry.value {
            Value::Scalar(text) | Value::LiteralBlock(text) if text.is_empty() => {
                write_trailing(&entry.trailing_comment, output);
                output.push('\n');
            }
            Value::LiteralBlock(text) => {
                write_literal(text, depth + 1, &entry.trailing_comment, output);
            }
            Value::Scalar(text) if needs_literal_block(text) => {
                write_literal(text, depth + 1, &entry.trailing_comment, output);
            }
            Value::Scalar(text) => {
                output.push(' ');
                output.push_str(text);
                write_trailing(&entry.trailing_comment, output);
                output.push('\n');
            }
            Value::Mapping(nested) => {
                write_trailing(&entry.trailing_comment, output);
                output.push('\n');
                write_mapping(nested, depth + 1, output);
            }
            Value::Sequence(items) => {
                write_trailing(&entry.trailing_comment, output);
                output.push('\n');
                write_sequence(items, depth + 1, output);
            }
        }
    }
}

fn write_sequence(items: &[SequenceItem], depth: usize, output: &mut String) {
    let pad = padding(depth);
    for item in items {
        write_comments(&item.leading_comments, &pad, output);
        match &item.value {
            Value::Scalar(text) | Value::LiteralBlock(text) if text.is_empty() => {
                output.push_str(&pad);
                output.push('-');
                write_trailing(&item.trailing_comment, output);
                output.push('\n');
            }
            Value::LiteralBlock(text) => {
                output.push_str(&pad);
                output.push('-');
                write_literal(text, depth + 1, &item.trailing_comment, output);
            }
            Value::Scalar(text) if needs_literal_block(text) || reads_as_entry(text) => {
                output.push_str(&pad);
                output.push('-');
                write_literal(text, depth + 1, &item.trailing_comment, output);
            }
            Value::Scalar(text) => {
                output.push_str(&pad);
                output.push_str("- ");
                output.push_str(text);
                write_trailing(&item.trailing_comment, output);
                output.push('\n');
            }
            Value::Mapping(entries) if entries.is_empty() => {
                output.push_str(&pad);
                output.push_str("-\n");
            }
            Value::Mapping(entries) => {
                write_compact_item(entries, &item.trailing_comment, depth, output)
            }
            // A sequence cannot start on the dash line (`- - x` reads as the
            // scalar "- x"), so a nested sequence goes one level in.
            Value::Sequence(nested) => {
                output.push_str(&pad);
                output.push_str("-\n");
                write_sequence(nested, depth + 1, output);
            }
        }
    }
}

/// `- key: value` with the remaining keys aligned under the first.
///
/// The mapping is rendered one level in and the first line's indentation is
/// swapped for the marker; `INDENT` is as wide as `- `, so every other line is
/// already in the right column. Comments above the first key are written above
/// the dash, because the dash line has no room for them. A comment at the end
/// of the dash line belongs to the item, and is written after the first key.
fn write_compact_item(
    entries: &[MappingEntry],
    trailing: &Option<String>,
    depth: usize,
    output: &mut String,
) {
    let pad = padding(depth);
    write_comments(&entries[0].leading_comments, &pad, output);
    let mut first = entries[0].clone();
    first.leading_comments.clear();
    if first.trailing_comment.is_none() {
        first.trailing_comment = trailing.clone();
    }
    let mut rendered = String::new();
    write_mapping(std::slice::from_ref(&first), depth + 1, &mut rendered);
    write_mapping(&entries[1..], depth + 1, &mut rendered);

    output.push_str(&pad);
    output.push_str("- ");
    output.push_str(&rendered[padding(depth + 1).len()..]);
}

/// The `|` header (the caller has written the `key:` or `-` before it) and the
/// body, each non-empty line indented to `depth`.
fn write_literal(text: &str, depth: usize, trailing: &Option<String>, output: &mut String) {
    output.push_str(" |");
    write_trailing(trailing, output);
    output.push('\n');
    let pad = padding(depth);
    for line in text.lines() {
        if !line.is_empty() {
            output.push_str(&pad);
            output.push_str(line);
        }
        output.push('\n');
    }
}

/// Whether a list item holding `text` would be read as a one-entry mapping.
///
/// With the compact form on (which every reader of these files uses), `- a: b`
/// is a mapping, so a scalar item that has a colon followed by a space, or
/// ends in a colon, has to go in a block. In a mapping value the same text is
/// plain (`k: a: b` is the scalar `a: b`), so only list items need this.
fn reads_as_entry(text: &str) -> bool {
    text.contains(": ") || text.ends_with(':')
}

/// Whether writing `text` inline would change how the parser reads it back.
fn needs_literal_block(text: &str) -> bool {
    // Several lines are the point of the literal form.
    text.contains('\n')
        || text.contains('\r')
        // Inline values are trimmed on reading, so edge whitespace would be lost.
        || text != text.trim()
        // A tab inside is easy to lose to an editor and legal only in a block.
        || text.contains('\t')
        // `#` after a space starts a comment; at the start it is a comment line.
        || text.contains(" #")
        || text.starts_with('#')
        // Block-scalar indicators; the rejected forms are hard parse errors.
        || text.starts_with('|')
        || text.starts_with('>')
        // Quotes are removed on reading, which would change the value.
        || text.starts_with('"')
        || text.starts_with('\'')
        // Anchors, aliases and tags are refused by the parser outright.
        || text.starts_with('*')
        || text.starts_with('&')
        || text.starts_with('!')
        // A leading fence opens an embedded document.
        || text.starts_with("```")
        // Read as another list item or a complex key.
        || text == "-"
        || text.starts_with("- ")
        || text == "?"
        || text.starts_with("? ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mapping_from, parse_root, EntriesMut};

    /// Compares what a consumer sees: literal blocks as plain scalars (their
    /// text without the final newline), source lines ignored.
    fn view(value: &Value) -> Value {
        match value {
            Value::Scalar(_) | Value::LiteralBlock(_) => {
                Value::Scalar(value.as_scalar().unwrap().to_string())
            }
            Value::Mapping(entries) => Value::Mapping(
                entries
                    .iter()
                    .map(|entry| MappingEntry {
                        line: 0,
                        key: entry.key.clone(),
                        value: view(&entry.value),
                        leading_comments: entry.leading_comments.clone(),
                        trailing_comment: entry.trailing_comment.clone(),
                    })
                    .collect(),
            ),
            Value::Sequence(items) => Value::Sequence(
                items
                    .iter()
                    .map(|item| SequenceItem {
                        line: 0,
                        value: view(&item.value),
                        leading_comments: item.leading_comments.clone(),
                        trailing_comment: item.trailing_comment.clone(),
                    })
                    .collect(),
            ),
        }
    }

    fn assert_round_trips(value: &Value) {
        let text = emit(value);
        let again = parse_root(&text)
            .unwrap_or_else(|error| panic!("emitted text failed to parse: {error}\n---\n{text}"));
        assert_eq!(
            view(&again),
            view(value),
            "round trip changed the tree\n---\n{text}"
        );
    }

    fn single(key: &str, value: Value) -> Value {
        Value::mapping(mapping_from([(key, value)]))
    }

    #[test]
    fn a_flat_mapping_keeps_its_key_order() {
        let value = Value::mapping(mapping_from([
            ("id", Value::scalar("018f2a7e")),
            ("state", Value::scalar("active")),
            ("assigned_to", Value::scalar("p3")),
        ]));
        assert_eq!(
            emit(&value),
            "id: 018f2a7e\nstate: active\nassigned_to: p3\n"
        );
    }

    #[test]
    fn an_empty_scalar_is_a_bare_key() {
        assert_eq!(emit(&single("notes", Value::scalar(""))), "notes:\n");
        assert_round_trips(&single("notes", Value::scalar("")));
    }

    #[test]
    fn multi_line_text_becomes_a_literal_block() {
        let value = single("description", Value::scalar("first line\nsecond line"));
        assert_eq!(
            emit(&value),
            "description: |\n  first line\n  second line\n"
        );
        assert_round_trips(&value);
    }

    #[test]
    fn blank_lines_inside_a_literal_block_survive() {
        let value = single("text", Value::scalar("a\n\nb"));
        assert_eq!(emit(&value), "text: |\n  a\n\n  b\n");
        assert_round_trips(&value);
    }

    #[test]
    fn a_nested_mapping_is_indented_two_spaces() {
        let inner = Value::mapping(mapping_from([("os", Value::scalar("Windows 11"))]));
        let value = single("environment", inner);
        assert_eq!(emit(&value), "environment:\n  os: Windows 11\n");
        assert_round_trips(&value);
    }

    #[test]
    fn empty_collections_are_bare_keys() {
        let value = Value::mapping(mapping_from([
            ("a", Value::mapping(Vec::new())),
            ("b", Value::sequence([])),
        ]));
        assert_eq!(emit(&value), "a:\nb:\n");
    }

    #[test]
    fn a_list_of_mappings_is_written_compactly() {
        let person = |id: &str, role: &str| {
            Value::mapping(mapping_from([
                ("id", Value::scalar(id)),
                ("role", Value::scalar(role)),
            ]))
        };
        let value = single(
            "people",
            Value::sequence([person("p1", "a"), person("p2", "b")]),
        );
        assert_eq!(
            emit(&value),
            "people:\n  - id: p1\n    role: a\n  - id: p2\n    role: b\n"
        );
        assert_round_trips(&value);
    }

    #[test]
    fn a_compact_item_may_hold_a_nested_list() {
        let tab = Value::mapping(mapping_from([
            ("side", Value::scalar("right")),
            (
                "tabs",
                Value::sequence([Value::scalar("one"), Value::scalar("two")]),
            ),
        ]));
        assert_round_trips(&single("panels", Value::sequence([tab])));
    }

    #[test]
    fn a_list_of_scalars_and_a_nested_list() {
        let value = single(
            "rows",
            Value::sequence([Value::scalar("a"), Value::sequence([Value::scalar("b")])]),
        );
        assert_round_trips(&value);
    }

    /// Each of these, written inline, would be read back as something else
    /// (a comment, a quoted string, a block header).
    #[test]
    fn hazardous_scalars_survive_a_round_trip() {
        for hazard in [
            "# not a comment",
            "| not an indicator",
            "> not a folded scalar",
            "```not a fence",
            "\"quoted\"",
            "'single'",
            "a # b",
            "a #",
            "*alias",
            "&anchor",
            "!tag",
            "see &name here",
            "- looks like an item",
            "-",
            "? looks like a complex key",
            "tab\tinside",
            "a: b",
        ] {
            assert_round_trips(&single("value", Value::scalar(hazard)));
            assert_round_trips(&Value::sequence([Value::scalar(hazard)]));
        }
    }

    /// A documented limit of the format, not of the writer: a block is
    /// dedented by its smallest indentation, so whitespace shared by every
    /// line is lost on reading.
    #[test]
    fn indentation_shared_by_every_line_is_lost() {
        let text = emit(&single("value", Value::scalar("  padded")));
        assert_eq!(text, "value: |\n    padded\n");
        let again = parse_root(&text).unwrap();
        assert_eq!(
            again
                .as_mapping()
                .unwrap()
                .get("value")
                .unwrap()
                .as_scalar(),
            Some("padded")
        );
    }

    #[test]
    fn comments_are_written_and_survive() {
        let text = "# about the file\nname: x # trailing\n# about count\ncount: 2\n";
        let parsed = parse_root(text).unwrap();
        assert_eq!(emit(&parsed), text);
    }

    #[test]
    fn a_trailing_comment_on_a_literal_block_header_survives() {
        let mut entry = MappingEntry::new("d", Value::scalar("a\nb"));
        entry.trailing_comment = Some("why".to_string());
        assert_round_trips(&Value::mapping(vec![entry]));
    }

    #[test]
    fn comments_on_the_first_key_of_a_compact_item_survive() {
        let text = "list:\n  # first\n  - id: p1 # one\n    role: a\n";
        let parsed = parse_root(text).unwrap();
        let again = parse_root(&emit(&parsed)).unwrap();
        assert_eq!(view(&again), view(&parsed));
    }

    #[test]
    fn emitting_what_was_parsed_changes_nothing() {
        let text = "title: hello\nenv:\n  os: x\nitems:\n  - id: p1\n    note: |\n      a\n      b\n  - id: p2\n    note:\n";
        let parsed = parse_root(text).unwrap();
        assert_eq!(emit(&parsed), text);
    }

    #[test]
    fn set_then_emit_keeps_untouched_keys_in_place() {
        let mut entries = mapping_from([("a", Value::scalar("1")), ("b", Value::scalar("2"))]);
        entries.set("a", Value::scalar("9"));
        assert_eq!(emit(&Value::mapping(entries)), "a: 9\nb: 2\n");
    }
}
