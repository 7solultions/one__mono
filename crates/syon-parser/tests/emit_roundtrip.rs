//! What the suite's data files need from the emitter: values that would be
//! read back as something else if written inline survive a round trip, and
//! comments survive. The emitter itself is `syon_parser::emitter`.

use syon_parser::{
    emit_with, mapping_from, parse_root, EmitOptions, EntriesMut, MappingEntry, SequenceItem,
    Value,
};

fn emit(value: &Value) -> String {
    emit_with(value, EmitOptions { compact_sequence_mappings: true })
}

/// What a consumer sees: `|` blocks as plain scalars (their text without the
/// final newline), source lines ignored.
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
    assert_eq!(view(&again), view(value), "round trip changed the tree\n---\n{text}");
}

fn single(key: &str, value: Value) -> Value {
    Value::mapping(mapping_from([(key, value)]))
}

#[test]
fn a_list_of_mappings_is_written_compactly() {
    let person = |id: &str, role: &str| {
        Value::mapping(mapping_from([("id", Value::scalar(id)), ("role", Value::scalar(role))]))
    };
    let value = single("people", Value::sequence([person("p1", "a"), person("p2", "b")]));
    assert_eq!(emit(&value), "people:\n  - id: p1\n    role: a\n  - id: p2\n    role: b\n");
    assert_round_trips(&value);
}

#[test]
fn a_compact_item_may_hold_a_nested_list() {
    let tab = Value::mapping(mapping_from([
        ("side", Value::scalar("right")),
        ("tabs", Value::sequence([Value::scalar("one"), Value::scalar("two")])),
    ]));
    assert_round_trips(&single("panels", Value::sequence([tab])));
}

/// Each of these, written inline, would be read back as something else.
#[test]
fn hazardous_scalars_survive_a_round_trip() {
    let mut failures: Vec<String> = Vec::new();
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
        "first\nsecond",
        "a\n\nb",
    ] {
        for value in [single("value", Value::scalar(hazard)), Value::sequence([Value::scalar(hazard)])] {
            let text = emit(&value);
            match parse_root(&text) {
                Ok(again) if view(&again) == view(&value) => {}
                Ok(again) => failures.push(format!("{hazard:?} comes back as {:?}", view(&again))),
                Err(error) => failures.push(format!("{hazard:?} is not readable again: {error}")),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn comments_survive() {
    let text = "# about the file\nname: x  # trailing\n# about count\ncount: 2\n";
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
fn set_then_emit_keeps_untouched_keys_in_place() {
    let mut entries = mapping_from([("a", Value::scalar("1")), ("b", Value::scalar("2"))]);
    entries.set("a", Value::scalar("9"));
    assert_eq!(emit(&Value::mapping(entries)), "a: 9\nb: 2\n");
}
