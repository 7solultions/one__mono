//! Rendering an AST back into SYON text.
//!
//! The inverse of [`crate::parser`], and it lives beside it for the same reason
//! the parser has no sibling: there is one implementation of SYON's syntax
//! here, so the block-only style and the promotion rules are decided in a
//! single place. Anything that persists SYON — a task runner, a project tree,
//! a schema — calls [`emit`] rather than assembling text by hand.
//!
//! ## What it guarantees
//!
//! * **Block style only.** No flow collection is ever produced, whatever a
//!   consumer put in a scalar.
//! * **Nothing needing a non-default permission.** A sequence of mappings is
//!   written with the dash on its own line rather than as `- key: value`,
//!   because the compact form is the "key in line after list" construct this
//!   parser refuses unless asked. An emitter whose output its own parser
//!   rejects by default would be a trap.
//! * **Order is preserved.** Mapping entries and sequence items are walked in
//!   the order they are held, so re-emitting an unchanged tree is byte-identical
//!   and an edit produces a minimal diff.
//! * **Comments survive.** This parser keeps `leading_comments` and
//!   `trailing_comment` on every entry and item, so a file that goes through
//!   [`crate::parse`] and back through [`emit`] keeps the prose somebody wrote
//!   in it. That is the part a consumer cannot reconstruct, and it is why an
//!   emitter belongs here rather than in each consumer.
//! * **Literal blocks where they are needed.** A scalar that cannot be written
//!   inline without changing meaning is promoted to a `|` block, so multi-line
//!   text lands in the file as readable lines.
//!
//! ## Round-trip limits, inherent to the format
//!
//! Three, and none of them is a defect in this module:
//!
//! * **Indentation common to every line of a block is not representable.** A
//!   `|` block is dedented to its own indentation on the way in, so `"  padded"`
//!   comes back as `"padded"`. *Relative* indentation survives, which is what
//!   matters for pasted logs and numbered steps.
//! * **A scalar ending in newlines** comes back with one, or none for a
//!   single-line value.
//! * **A scalar that is exactly `|` or `>`** cannot be written as a scalar: on
//!   its own after a key, either opens a block. It is promoted, so the text
//!   survives, but it reads back as a `LiteralBlock` rather than a `Scalar`.
//! * **A key containing `:` or starting with `#`** has no spelling. Keys in
//!   practice are identifiers, so they are written verbatim rather than
//!   failing; validate keys upstream if they come from user input.
//!
//! And one that is worth stating separately, because it is about the file
//! rather than about a value:
//!
//! * **Several *unfenced* documents cannot be written back as several.** The
//!   parser produces one when a construct at the same indentation as its key
//!   turns out to be a sibling rather than a child, so a file can hold two
//!   without ever having named a fence. There is no separator to write between
//!   them — that is what a fence is for — so [`emit_file`] concatenates them
//!   and the result reads back as one. Emitting a tree that was fenced is
//!   exact; emitting one that was accidentally split is lossy, and the fix is
//!   to fence the source. [`emit_file`] cannot tell the difference, so
//!   [`is_faithful`] is what a caller checks when it matters.

use crate::ast::{Document, MappingEntry, SequenceItem, SyonFile, Value};

/// How wide one level of indentation is.
///
/// Two spaces, matching [`crate::ParseOptions::space_count`]'s default and the
/// width of a `- ` marker — so a sequence item's continuation lines line up
/// under its first without special handling.
const INDENT: &str = "  ";

/// The marker a document fence opens with.
const FENCE_OPEN: &str = "---";

/// The marker a document fence closes with. Optional in the grammar; written
/// anyway, because a reader should not have to look ahead to find where a
/// document ends.
const FENCE_CLOSE: &str = "...";

/// How to write. Every option trades a shape a reader prefers against a
/// permission the reader's parser has to be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmitOptions {
    /// Write a sequence of mappings as `- key: value` rather than putting the
    /// dash on a line of its own.
    ///
    /// **Off by default, and the default is the safe one.** The compact form is
    /// a line shorter and much easier to read — it is how a reference like
    ///
    /// ```text
    /// feeds_gate:
    ///   - id: 01a054a4-72aa-7ff2-8c83-fcdd9867e2e7
    ///     alias: Release 1.0
    /// ```
    ///
    /// is naturally written. It is also the "key in line after list" construct,
    /// which [`crate::parse`] refuses unless
    /// [`crate::ParseOptions::allow_key_in_line_after_list`] is set. So a
    /// producer may only turn this on if it knows every consumer of its files
    /// reads them with that permission — which is a claim about a whole
    /// ecosystem, not about one call site, and is why it is not the default.
    pub compact_sequence_mappings: bool,
}

impl Default for EmitOptions {
    /// Output that [`crate::parse`] accepts with no permissions granted.
    fn default() -> Self {
        EmitOptions {
            compact_sequence_mappings: false,
        }
    }
}

/// Whether [`emit_file`] can reproduce this file's document structure.
///
/// `false` for a file holding more than one *unfenced* document, which has no
/// separator to write between them. See the round-trip limits above. A caller
/// that must not lose structure checks this before writing.
pub fn is_faithful(file: &SyonFile) -> bool {
    let unfenced = file
        .documents
        .iter()
        .filter(|document| document.path.is_none() && document.format.is_none())
        .count();
    unfenced <= 1
}

/// Renders a whole file, fences and all.
pub fn emit_file(file: &SyonFile) -> String {
    emit_file_with(file, EmitOptions::default())
}

/// Renders a whole file with explicit options.
pub fn emit_file_with(file: &SyonFile, options: EmitOptions) -> String {
    let mut out = String::new();
    for (index, document) in file.documents.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        emit_document_into(document, options, &mut out);
    }
    out
}

/// Renders one document.
pub fn emit_document(document: &Document) -> String {
    let mut out = String::new();
    emit_document_into(document, EmitOptions::default(), &mut out);
    out
}

/// Renders a bare value, with no document fence around it.
///
/// What a consumer holding a `Value` wants — a project file, a task
/// definition — where the fence machinery is not in play.
pub fn emit(value: &Value) -> String {
    emit_with(value, EmitOptions::default())
}

/// Renders a bare value with explicit options.
pub fn emit_with(value: &Value, options: EmitOptions) -> String {
    let mut out = String::new();
    emit_value(value, 0, options, &mut out);
    out
}

fn emit_document_into(document: &Document, options: EmitOptions, out: &mut String) {
    // A document carries a fence only when it was introduced by one. The first
    // document in a file usually was not, and wrapping it would change what the
    // file means.
    let fenced = document.path.is_some() || document.format.is_some();
    if fenced {
        out.push_str(FENCE_OPEN);
        out.push_str(document.path.as_deref().unwrap_or(""));
        out.push('.');
        out.push_str(document.format.as_deref().unwrap_or(""));
        out.push('\n');
    }
    emit_value(&document.body, 0, options, out);
    if fenced {
        out.push_str(FENCE_CLOSE);
        out.push('\n');
    }
}

fn emit_value(value: &Value, depth: usize, options: EmitOptions, out: &mut String) {
    match value {
        Value::Mapping(entries) => emit_mapping(entries, depth, options, out),
        Value::Sequence(items) => emit_sequence(items, depth, options, out),
        Value::LiteralBlock(text) => emit_block_body(text, depth, out),
        Value::Scalar(text) => {
            let padding = INDENT.repeat(depth);
            for line in text.lines() {
                out.push_str(&padding);
                out.push_str(line);
                out.push('\n');
            }
        }
    }
}

fn emit_mapping(entries: &[MappingEntry], depth: usize, options: EmitOptions, out: &mut String) {
    let padding = INDENT.repeat(depth);
    for entry in entries {
        emit_comments(&entry.leading_comments, &padding, out);

        out.push_str(&padding);
        out.push_str(&entry.key);
        out.push(':');

        match &entry.value {
            Value::Scalar(text) if text.is_empty() => {
                emit_trailing(&entry.trailing_comment, out);
            }
            Value::Scalar(text) if needs_block(text) => {
                out.push_str(" |");
                emit_trailing(&entry.trailing_comment, out);
                emit_block_body(text, depth + 1, out);
            }
            Value::Scalar(text) => {
                out.push(' ');
                out.push_str(text);
                emit_trailing(&entry.trailing_comment, out);
            }
            Value::LiteralBlock(text) => {
                out.push_str(" |");
                emit_trailing(&entry.trailing_comment, out);
                emit_block_body(text, depth + 1, out);
            }
            Value::Mapping(nested) => {
                emit_trailing(&entry.trailing_comment, out);
                emit_mapping(nested, depth + 1, options, out);
            }
            Value::Sequence(items) => {
                emit_trailing(&entry.trailing_comment, out);
                emit_sequence(items, depth + 1, options, out);
            }
        }
    }
}

fn emit_sequence(items: &[SequenceItem], depth: usize, options: EmitOptions, out: &mut String) {
    let padding = INDENT.repeat(depth);
    for item in items {
        emit_comments(&item.leading_comments, &padding, out);

        match &item.value {
            Value::Scalar(text) if text.is_empty() => {
                out.push_str(&padding);
                out.push('-');
                emit_trailing(&item.trailing_comment, out);
            }
            // `- - a` would open a nested sequence rather than hold the text.
            Value::Scalar(text)
                if needs_block(text)
                    || text.starts_with("- ")
                    || (options.compact_sequence_mappings && reads_as_entry(text)) =>
            {
                out.push_str(&padding);
                out.push_str("- |");
                emit_trailing(&item.trailing_comment, out);
                emit_block_body(text, depth + 1, out);
            }
            Value::Scalar(text) => {
                out.push_str(&padding);
                out.push_str("- ");
                out.push_str(text);
                emit_trailing(&item.trailing_comment, out);
            }
            Value::LiteralBlock(text) => {
                out.push_str(&padding);
                out.push_str("- |");
                emit_trailing(&item.trailing_comment, out);
                emit_block_body(text, depth + 1, out);
            }
            Value::Mapping(nested) if nested.is_empty() => {
                out.push_str(&padding);
                out.push('-');
                emit_trailing(&item.trailing_comment, out);
            }
            Value::Sequence(nested) if nested.is_empty() => {
                out.push_str(&padding);
                out.push('-');
                emit_trailing(&item.trailing_comment, out);
            }
            // The dash goes on a line of its own and the structure is
            // indented beneath it.
            //
            // The tempting alternative is to splice the first key onto the
            // dash — `- name: …` — which is a line shorter and reads better.
            // It is also the "key in line after list" construct, which this
            // parser refuses unless `allow_key_in_line_after_list` is set. An
            // emitter that writes files its own parser rejects by default is
            // worse than one that writes a plainer file, so the plainer file
            // wins. Every `.syon` in this repository is written this way too.
            // A mapping may be spliced onto the dash when the caller has said
            // its readers allow it; anything else gets the plain form.
            Value::Mapping(nested) if options.compact_sequence_mappings => {
                let mut rendered = String::new();
                emit_mapping(nested, depth + 1, options, &mut rendered);
                splice_marker(&padding, &rendered, &item.trailing_comment, out);
            }
            nested => {
                out.push_str(&padding);
                out.push('-');
                emit_trailing(&item.trailing_comment, out);
                emit_value(nested, depth + 1, options, out);
            }
        }
    }
}

/// Writes `rendered` with its first line's indentation replaced by `- `.
///
/// `INDENT` is exactly as wide as `- `, so every continuation line already sits
/// in the right column.
fn splice_marker(padding: &str, rendered: &str, trailing: &Option<String>, out: &mut String) {
    let mut lines = rendered.lines();
    if let Some(first) = lines.next() {
        let body = first
            .strip_prefix(padding)
            .and_then(|rest| rest.strip_prefix(INDENT))
            .unwrap_or(first);
        out.push_str(padding);
        out.push_str("- ");
        out.push_str(body);
        emit_trailing(trailing, out);
    }
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
}

/// Writes full-line comments above the thing they belong to.
///
/// Stored without their `#`, so it is put back here — and with a space after
/// it, which the grammar requires for a comment carrying any text.
fn emit_comments(comments: &[String], padding: &str, out: &mut String) {
    for comment in comments {
        out.push_str(padding);
        if comment.is_empty() {
            out.push_str("#\n");
        } else {
            out.push_str("# ");
            out.push_str(comment);
            out.push('\n');
        }
    }
}

/// Ends the current line, with a trailing comment if there is one.
fn emit_trailing(comment: &Option<String>, out: &mut String) {
    match comment {
        Some(text) if !text.is_empty() => {
            out.push_str("  # ");
            out.push_str(text);
        }
        Some(_) => out.push_str("  #"),
        None => {}
    }
    out.push('\n');
}

/// Writes the body of a `|` block, each line at `depth`.
fn emit_block_body(text: &str, depth: usize, out: &mut String) {
    let padding = INDENT.repeat(depth);
    for line in text.lines() {
        // An empty line takes no indentation: trailing spaces on a blank line
        // are invisible in the file and noise in a diff.
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&padding);
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// Whether a scalar has to be written as a `|` block.
///
/// Every case here is one where writing the text inline would read back as
/// something else.
fn needs_block(text: &str) -> bool {
    // Multi-line text is what the block form is for.
    text.contains('\n')
        // Inline scalars are trimmed, so surrounding whitespace would be lost.
        || text != text.trim()
        // A `#` at the start opens a comment.
        || text.starts_with('#')
        // A trailing comment is `SP+ "#" (SP text | end-of-line)`, so only a
        // hash that is *both* preceded by a space and followed by one — or by
        // the end — starts one. A bare `#2` in the middle of a sentence does
        // not, and promoting it to a block would turn prose into a block
        // scalar for nothing. There is a real ADR in this repository that says
        // "see issues #2 and #3", which is how this was found.
        || text.contains(" # ")
        || text.ends_with(" #")
        // `|` and `>` open a block only when they are the *whole* value.
        // `>=1.89` and `| inline` read back as ordinary scalars, and promoting
        // them would turn a version requirement into a block scalar. There is
        // a `require: '>=1.89'` in this repository's own task file, which is
        // how the over-eager version of this check was caught.
        || text == "|"
        || text == ">"
        // Quotes are taken off a quoted scalar on the way in, so a value that
        // itself starts with one would come back without it.
        || text.starts_with('"')
        || text.starts_with('\'')
        // Anchors, aliases and tags are refused by the parser, and not only at
        // the start of a line: `&name`, `*name` and `!name` are rejected where
        // a value begins. A `*` or `&` followed by a space is ordinary text.
        || starts_reference(text)
}

/// Whether `text` begins with an anchor, alias or tag the parser forbids.
fn starts_reference(text: &str) -> bool {
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some('&' | '*'), Some(next)) => next.is_ascii_alphanumeric(),
        (Some('!'), Some(next)) => next.is_ascii_alphanumeric() || next == '!',
        _ => false,
    }
}

/// Whether a list item holding `text` would be read as a one-entry mapping.
///
/// Only when readers have the compact permission on: then `- a: b` is a
/// mapping, so a scalar item with a colon and a space in it has to go in a
/// block. In a mapping value the same text is plain (`k: a: b`).
fn reads_as_entry(text: &str) -> bool {
    text.contains(": ") || text.ends_with(':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse, ParseOptions};

    /// Parses, emits, parses again, and insists the two trees agree.
    ///
    /// The property that matters: not that the text is identical — a file can
    /// be indented four spaces and come back indented two — but that nothing
    /// the parser can see has changed.
    fn round_trip(source: &str) -> SyonFile {
        let first = parse(source).unwrap_or_else(|e| panic!("parsing:\n{source}\n{e}"));
        let text = emit_file(&first);
        let second =
            parse(&text).unwrap_or_else(|e| panic!("re-parsing what was emitted:\n{text}\n{e}"));
        assert_eq!(first, second, "emitted:\n{text}");
        second
    }

    #[test]
    fn a_mapping_round_trips() {
        let file = round_trip("kind: gate\nid: 01926a3e\nstatus: open\n");
        let Value::Mapping(entries) = &file.documents[0].body else {
            panic!("expected a mapping");
        };
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].key, "kind");
    }

    #[test]
    fn nesting_and_sequences_round_trip() {
        round_trip(
            "\
feeds_gate:
  -
    id: 01926a3e
    alias: Release 1.0
rasci:
  responsible: alice
  consulted:
    - security.team
    - product.owner
",
        );
    }

    #[test]
    fn an_unchanged_tree_re_emits_byte_for_byte() {
        // What makes a diff minimal: emitting twice changes nothing.
        let source = "kind: gate\naliases:\n  - Release 1.0\nstatus: open\n";
        let once = emit_file(&parse(source).unwrap());
        let twice = emit_file(&parse(&once).unwrap());
        assert_eq!(once, twice);
        assert_eq!(once, source, "and this input was already canonical");
    }

    #[test]
    fn comments_survive_the_round_trip() {
        // The part a consumer cannot reconstruct, and the reason the emitter
        // belongs in the parser crate rather than in each consumer.
        let source = "\
# what this file is
# on two lines
kind: gate  # and one at the end
status: open
";
        let file = round_trip(source);
        let Value::Mapping(entries) = &file.documents[0].body else {
            panic!("expected a mapping");
        };
        assert_eq!(
            entries[0].leading_comments,
            vec!["what this file is", "on two lines"]
        );
        assert_eq!(
            entries[0].trailing_comment.as_deref(),
            Some("and one at the end")
        );
        assert_eq!(emit_file(&file), source);
    }

    #[test]
    fn a_comment_on_a_sequence_item_survives_too() {
        round_trip(
            "\
steps:
  # the first one
  -
    id: 01926a40  # by name
  -
    id: 01926a41
",
        );
    }

    #[test]
    fn a_literal_block_stays_a_literal_block() {
        let file = round_trip("notes: |\n  first line\n  second line\n");
        let Value::Mapping(entries) = &file.documents[0].body else {
            panic!("expected a mapping");
        };
        assert!(matches!(entries[0].value, Value::LiteralBlock(_)));
    }

    #[test]
    fn a_blank_line_inside_a_block_keeps_no_trailing_spaces() {
        let text = emit(&Value::Mapping(vec![entry(
            "notes",
            Value::LiteralBlock("first\n\nthird\n".to_string()),
        )]));
        assert!(text.contains("\n\n  third"), "got:\n{text:?}");
        assert!(!text.contains("  \n"), "a blank line was padded: {text:?}");
    }

    #[test]
    fn multi_line_text_is_promoted_to_a_block_rather_than_written_inline() {
        let text = emit(&Value::Mapping(vec![entry(
            "notes",
            Value::Scalar("first line\nsecond line".to_string()),
        )]));
        assert!(text.starts_with("notes: |\n"), "got:\n{text}");

        let file = parse(&text).unwrap();
        let Value::Mapping(entries) = &file.documents[0].body else {
            panic!("expected a mapping");
        };
        assert_eq!(
            entries[0].value,
            Value::LiteralBlock("first line\nsecond line\n".to_string())
        );
    }

    #[test]
    fn text_that_would_open_a_comment_is_promoted() {
        for awkward in ["# not a comment", "value  # not trailing"] {
            let text = emit(&Value::Mapping(vec![entry(
                "notes",
                Value::Scalar(awkward.to_string()),
            )]));
            let file = parse(&text).unwrap_or_else(|e| panic!("{awkward:?} emitted badly: {e}"));
            let Value::Mapping(entries) = &file.documents[0].body else {
                panic!("expected a mapping");
            };
            let read = match &entries[0].value {
                Value::Scalar(text) | Value::LiteralBlock(text) => text.trim_end().to_string(),
                other => panic!("expected text, got {other:?}"),
            };
            assert_eq!(read, awkward, "from:\n{text}");
        }
    }

    #[test]
    fn only_a_bare_block_indicator_is_promoted() {
        for awkward in ["|", ">", "  padded"] {
            let text = emit(&Value::Mapping(vec![entry(
                "notes",
                Value::Scalar(awkward.to_string()),
            )]));
            let file = parse(&text).unwrap_or_else(|e| panic!("{awkward:?} emitted badly: {e}"));
            let Value::Mapping(entries) = &file.documents[0].body else {
                panic!("expected a mapping");
            };
            // Leading whitespace is the one loss the format cannot avoid.
            let read = match &entries[0].value {
                Value::Scalar(text) | Value::LiteralBlock(text) => text.trim_end().to_string(),
                other => panic!("expected text, got {other:?}"),
            };
            assert_eq!(read, awkward.trim(), "from:\n{text}");
        }
    }

    #[test]
    fn a_sequence_item_that_starts_with_a_dash_is_promoted() {
        // `- - a` would open a nested sequence rather than hold the text.
        let text = emit(&Value::Sequence(vec![item(Value::Scalar(
            "- looks like a list".to_string(),
        ))]));
        let file = parse(&text).unwrap();
        let Value::Sequence(items) = &file.documents[0].body else {
            panic!("expected a sequence");
        };
        let read = match &items[0].value {
            Value::Scalar(text) | Value::LiteralBlock(text) => text.trim_end().to_string(),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(read, "- looks like a list");
    }

    #[test]
    fn a_document_fence_round_trips_with_its_path_and_format() {
        let source = "---notes/today.md\ncontent: something\n...\n";
        let file = parse(source).unwrap_or_else(|e| panic!("{e}"));
        let emitted = emit_file(&file);
        assert_eq!(parse(&emitted).unwrap(), file, "emitted:\n{emitted}");
        assert!(emitted.starts_with("---notes/today"), "got:\n{emitted}");
        assert!(emitted.trim_end().ends_with("..."), "got:\n{emitted}");
    }

    #[test]
    fn an_unfenced_document_gains_no_fence() {
        // Wrapping the first document in one would change what the file means.
        let emitted = emit_file(&parse("kind: gate\n").unwrap());
        assert!(!emitted.starts_with(FENCE_OPEN), "got:\n{emitted}");
    }

    /// Line numbers move when a file is re-emitted — a blank line goes, an
    /// indent narrows — and none of that is a change in meaning. They are
    /// zeroed before comparing so the test is about content.
    fn forget_lines(file: &mut SyonFile) {
        fn walk(value: &mut Value) {
            match value {
                Value::Mapping(entries) => {
                    for entry in entries {
                        entry.line = 0;
                        walk(&mut entry.value);
                    }
                }
                Value::Sequence(items) => {
                    for item in items {
                        item.line = 0;
                        walk(&mut item.value);
                    }
                }
                _ => {}
            }
        }
        for document in &mut file.documents {
            walk(&mut document.body);
        }
    }

    #[test]
    fn every_syon_file_in_this_repository_round_trips() {
        // The real proof. Anything the emitter cannot reproduce shows up here
        // against files nobody wrote for this test — which is how the `#2`
        // over-promotion and the `- key:` compact form were both found.
        let mut checked = 0;
        let mut unfaithful = 0;
        for path in syon_files() {
            let source = std::fs::read_to_string(&path).expect("readable");
            let Ok(mut first) = parse(&source) else {
                // Some fixtures are deliberately invalid; they are the
                // parser's business, not the emitter's.
                continue;
            };
            if !is_faithful(&first) {
                // Several unfenced documents, which have no separator. A
                // documented limit rather than a defect — see the module docs.
                unfaithful += 1;
                continue;
            }

            let emitted = emit_file(&first);
            let mut second = parse(&emitted)
                .unwrap_or_else(|e| panic!("{} did not survive:\n{emitted}\n{e}", path.display()));
            forget_lines(&mut first);
            forget_lines(&mut second);
            assert_eq!(first, second, "{} changed meaning", path.display());
            checked += 1;
        }
        assert!(
            checked > 20,
            "only {checked} files were checked; the walk found too little"
        );
        // If this ever climbs, the format has grown a way to be split by
        // accident and the limit is worth revisiting rather than tolerating.
        assert!(
            unfaithful <= 2,
            "{unfaithful} files hold several unfenced documents"
        );
    }

    #[test]
    fn the_compact_form_is_off_by_default_and_on_when_asked() {
        // The shape Kadima writes its references in. Readable, and it needs a
        // permission — which is exactly why it is opt-in.
        let tree = Value::Mapping(vec![entry(
            "feeds_gate",
            Value::Sequence(vec![item(Value::Mapping(vec![
                entry("id", Value::Scalar("01a054a4".to_string())),
                entry("alias", Value::Scalar("Release 1.0".to_string())),
            ]))]),
        )]);

        let plain = emit(&tree);
        assert!(plain.contains("  -\n    id: 01a054a4"), "got:\n{plain}");
        // The default output parses with nothing granted.
        assert!(parse(&plain).is_ok(), "the safe form needs no permission");

        let compact = emit_with(
            &tree,
            EmitOptions {
                compact_sequence_mappings: true,
            },
        );
        assert!(compact.contains("  - id: 01a054a4"), "got:\n{compact}");
        assert!(
            compact.contains("    alias: Release 1.0"),
            "got:\n{compact}"
        );
    }

    #[test]
    fn the_compact_form_needs_the_permission_it_documents() {
        // Stated as a test so nobody flips the default without meeting this.
        let tree = Value::Mapping(vec![entry(
            "feeds_gate",
            Value::Sequence(vec![item(Value::Mapping(vec![
                entry("id", Value::Scalar("01a054a4".to_string())),
                entry("alias", Value::Scalar("Release 1.0".to_string())),
            ]))]),
        )]);
        let compact = emit_with(
            &tree,
            EmitOptions {
                compact_sequence_mappings: true,
            },
        );

        assert!(parse(&compact).is_err(), "default options must refuse it");

        let permitted = ParseOptions {
            strict: false,
            allow_key_in_line_after_list: true,
            ..Default::default()
        };
        let mut read = crate::parse_with(&compact, permitted).expect("permitted options read it");
        // The hand-built tree carries no line numbers, and the parsed one does.
        forget_lines(&mut read);
        assert_eq!(read.documents[0].body, tree);
    }

    #[test]
    fn a_file_split_into_several_unfenced_documents_is_reported_as_unfaithful() {
        // A sequence at the same indentation as its key is a sibling, not a
        // child, so this file holds two documents without naming a fence.
        let split = parse("cmds:\n- one\n- two\n").expect("parses");
        assert!(
            split.documents.len() > 1,
            "the parser splits this: {split:?}"
        );
        assert!(!is_faithful(&split), "and the emitter cannot put it back");

        assert!(is_faithful(&parse("cmds:\n  - one\n").unwrap()));
    }

    fn syon_files() -> Vec<std::path::PathBuf> {
        fn walk(at: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(at) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if path.is_dir() {
                    if !matches!(name, "target" | ".git" | "node_modules") {
                        walk(&path, found);
                    }
                } else if name.ends_with(".syon") {
                    found.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .expect("the repository root")
            .to_path_buf();
        let mut found = Vec::new();
        walk(&root, &mut found);
        found
    }

    #[test]
    fn the_indent_width_matches_what_the_parser_expects_by_default() {
        assert_eq!(INDENT.len(), ParseOptions::default().space_count);
    }

    fn entry(key: &str, value: Value) -> MappingEntry {
        MappingEntry {
            line: 0,
            key: key.to_string(),
            value,
            leading_comments: Vec::new(),
            trailing_comment: None,
        }
    }

    fn item(value: Value) -> SequenceItem {
        SequenceItem {
            line: 0,
            value,
            leading_comments: Vec::new(),
            trailing_comment: None,
        }
    }
}
