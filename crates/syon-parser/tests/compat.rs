//! Text written by the old (serde_yaml-era) emitter must read under this parser
//! and be written back unchanged, so a consumer that loads and saves a file
//! leaves no diff.

use syon_parser::{emit_with, parse_root, EmitOptions, SequenceItems, Value};

/// The form existing files are written in: lists of mappings on the dash line.
fn emit(value: &Value) -> String {
    emit_with(value, EmitOptions { compact_sequence_mappings: true })
}

/// Roster files as the old emitter wrote them: compact lists of mappings,
/// `|` blocks, empty values.
const FIXTURES: [(&str, &str); 3] = [
    ("stadler-rail", include_str!("fixtures/stadler-rail.syon")),
    ("spiraya", include_str!("fixtures/spiraya.syon")),
    (
        "roster_demo_rail",
        include_str!("fixtures/roster_demo_rail.syon"),
    ),
];

const NESTED: &str = "environment:\n  os: Windows 11\n  arch: x86_64\nparticipants:\n  - id: p1\n    company: stadler-rail\n    role: reporter\n  - id: p2\n    company: spiraya\n    role: handler\n";
const BLOCK: &str = "description: |\n  The application crashes whenever I try to save\n  a file with a very long name.\n";

#[test]
fn files_written_by_the_old_emitter_re_emit_byte_for_byte() {
    for (name, text) in FIXTURES.iter().copied().chain([
        ("nested", NESTED),
        ("block", BLOCK),
        ("empty value", "notes:\n"),
        ("flat", "title: hello\nseverity: high\n"),
    ]) {
        let value = parse_root(text).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(emit(&value), text, "{name} changed when read and written");
    }
}

#[test]
fn the_roster_shape_reads_as_the_consumers_expect() {
    let root = parse_root(NESTED).unwrap();
    let entries = root.as_mapping().unwrap();
    let people = entries.get("participants").unwrap().as_sequence().unwrap();
    let ids: Vec<_> = people
        .values()
        .map(|person| {
            person
                .as_mapping()
                .unwrap()
                .get("id")
                .unwrap()
                .as_scalar()
                .unwrap()
        })
        .collect();
    assert_eq!(ids, vec!["p1", "p2"]);
}

#[test]
fn a_block_reads_without_its_final_newline() {
    let root = parse_root(BLOCK).unwrap();
    let text = root
        .as_mapping()
        .unwrap()
        .get("description")
        .unwrap()
        .as_scalar()
        .unwrap();
    assert_eq!(
        text,
        "The application crashes whenever I try to save\na file with a very long name."
    );
}

#[test]
fn duplicate_keys_are_rejected() {
    assert!(parse_root("a: 1\na: 2\n").is_err());
}

#[test]
fn an_empty_file_is_an_empty_mapping() {
    assert_eq!(parse_root("").unwrap(), Value::mapping(Vec::new()));
}
