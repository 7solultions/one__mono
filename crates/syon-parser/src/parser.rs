use pest::{iterators::Pair, Parser};
use pest_derive::Parser;

use crate::ast::{Document, MappingEntry, SequenceItem, SyonFile, Value};
use crate::error::SyonError;
use crate::error_code::ErrorCode;

#[derive(Parser)]
#[grammar = "src/grammar.pest"]
pub struct SyonParser;

// ---------------------------------------------------------------------------
// Forbidden-construct pre-flight scan
// ---------------------------------------------------------------------------

/// Whether the parser may interpret flow collections, rather than leaving
/// them as text.
///
/// Flow collections are never *rejected*. `[a, b]` and `{k: v}` are always
/// accepted; the only question is whether this layer turns them into a
/// sequence or mapping, or hands them onward as the scalar `"[a, b]"` for the
/// consumer to interpret. Declining to interpret is the default, and is what
/// "safe" means in SYON: the generic parser does not decide what application
/// syntax means.
///
/// Anchors, aliases and tags are a separate matter and are always rejected.
/// They are the parser's own reference and typing machinery, so honouring
/// them *is* interpretation, and no option enables it.
///
/// [`strict`](Self::strict) is the master switch: while it is set nothing is
/// interpreted, whatever the individual flags say. Enabling interpretation
/// therefore takes two deliberate steps.
///
/// ```
/// use syon_parser::parser::ParseOptions;
///
/// // Default: `[a, b]` arrives as the string "[a, b]".
/// let safe = ParseOptions::default();
///
/// // Opt in to sequences only; `{k: v}` still arrives as text.
/// let lists = ParseOptions::lists_only();
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Master switch. While true nothing is interpreted and both `allow_*`
    /// flags have no effect.
    pub strict: bool,
    /// Interpret `[a, b]` as a sequence. Requires `strict: false`.
    pub allow_list: bool,
    /// Interpret `{k: v}` as a mapping. Requires `strict: false`.
    ///
    /// Worth leaving off: a flow mapping is the more opaque of the two, and
    /// it is far rarer in practice.
    pub allow_dictionary: bool,
    /// Spaces per indentation level.
    ///
    /// Structural lines -- mapping entries and sequence items -- must be
    /// indented by a whole multiple of this, which makes nesting depth a
    /// property the tokeniser can compute (`indent / space_count`) rather
    /// than something only the tree builder can infer. Ragged indentation
    /// becomes an error instead of a surprising tree.
    ///
    /// Verbatim regions are exempt: the body of a `|` block and folded
    /// continuation lines are content, and are routinely aligned to columns
    /// that suit the reader.
    pub space_count: usize,
    /// Read `- key: value` as a compact mapping rather than as scalar text.
    ///
    /// YAML reads that line as a one-entry mapping, and Taskfiles rely on it
    /// (`- task: build`). But ordinary prose has the same shape -- `- Note:
    /// see the appendix` is a sentence, not a mapping -- so interpreting it
    /// is a choice about the document's dialect, and the safe default is to
    /// leave the text alone.
    ///
    /// A compact entry with nothing after its colon takes the lines indented
    /// past its key as its value, and the lines aligned with the key as
    /// further entries -- so `- tabs:` can hold a nested list next to
    /// `side: right`, exactly as the expanded form does.
    ///
    /// While this is off, a sequence item that has *both* inline text and a
    /// deeper block is an error rather than silently losing one of them.
    pub allow_key_in_line_after_list: bool,
}

impl Default for ParseOptions {
    /// Interpret nothing: flow collections arrive as text.
    fn default() -> Self {
        Self {
            strict: true,
            allow_list: false,
            allow_dictionary: false,
            space_count: 2,
            allow_key_in_line_after_list: false,
        }
    }
}

impl ParseOptions {
    /// Interpret flow sequences, but not flow mappings.
    pub fn lists_only() -> Self {
        Self { strict: false, allow_list: true, allow_dictionary: false, ..Self::default() }
    }

    /// Interpret both flow forms.
    pub fn permissive() -> Self {
        Self { strict: false, allow_list: true, allow_dictionary: true, ..Self::default() }
    }

    /// Whether a flow collection opened by `opener` should be interpreted.
    #[allow(dead_code)]
    fn interprets_flow(&self, opener: char) -> bool {
        if self.strict {
            return false;
        }
        match opener {
            '[' => self.allow_list,
            '{' => self.allow_dictionary,
            _ => false,
        }
    }
}

/// Indentation of a `|` block scalar header line, if this line is one.
///
/// Preflight runs before the grammar, so it has to recognise a header
/// textually. The token must be the whole value, matching what
/// [`block_header`] accepts once the grammar has split the line: `cmd: echo a |`
/// is a scalar that happens to end in a pipe, not a header.
fn block_header_line(line: &str) -> Option<usize> {
    let t = line.trim_end();
    let before = ["|-", "|+", ">-", ">+", "|", ">"]
        .iter()
        .find_map(|tok| t.strip_suffix(tok))?;
    // The token is the whole value: it either stands alone on the line, or
    // follows a `: ` or `- ` operator.
    let is_value =
        before.trim_start().is_empty() || before.ends_with(": ") || before.ends_with("- ");
    is_value.then(|| line.len() - line.trim_start().len())
}

/// The info string on a document-fence opener: everything after `---` on the
/// line. Empty for a bare `---`, which is a YAML stream marker rather than a
/// fence, and is what keeps the two apart.
fn fence_info_string(line: &str) -> &str {
    line.strip_prefix("---").unwrap_or("").trim_end()
}

fn preflight(input: &str) -> Result<(), SyonError> {
    let mut seen_content = false;
    // Line number of the currently-open ``` fence, if any. Tracked here
    // rather than in the grammar because `fence_open` and `fence_close` are
    // independent rules -- pest will happily match an open with no close, and
    // the builder then treats the rest of the file as that document's body.
    let mut open_fence: Option<usize> = None;
    // Indent of the `|` block scalar header whose body we are inside, if any.
    // A body is verbatim text: `---` there is three dashes the author wrote,
    // not a document marker, and giving it structural meaning rejects valid
    // documents. syon-go already reads it that way.
    let mut block_indent: Option<usize> = None;

    for (i, line) in input.lines().enumerate() {
        let ln = i + 1;
        let t = line.trim_start();

        // Tabs anywhere in the leading whitespace are forbidden (SYON uses spaces only).
        if line.chars().take_while(|c| c.is_whitespace()).any(|c| c == '\t') {
            return Err(SyonError::syntax(
                ErrorCode::TabInIndentation,
                format!("line {ln}: tab character in indentation is not allowed in SYON (use spaces)"),
            ));
        }

        // Inside a block scalar body nothing below applies -- the body is
        // content. The body runs to the first non-blank line indented no
        // further than the header; blank lines never end it.
        if let Some(header_indent) = block_indent {
            if t.is_empty() || line.len() - t.len() > header_indent {
                continue;
            }
            block_indent = None;
        }
        // Set for the lines that follow: the header line itself is still
        // structure, and falls through to the checks below.
        if let Some(indent) = block_header_line(line) {
            block_indent = Some(indent);
        }

        // Document fences (Block 2), column 0 only, and handled before the
        // stream-marker checks below so the two cannot be confused.
        //
        // A fence opens with `---` carrying an info string and closes with
        // `...`. The close is optional: a fence that runs to end of file is
        // complete, exactly as a YAML document is. That is why there is no
        // unterminated-fence error any more -- there is no such thing.
        //
        // Inside a fence the body is another media type entirely, so none of
        // SYON's forbidden-construct checks apply to it.
        if open_fence.is_some() {
            if t == "..." {
                open_fence = None;
            }
            continue;
        }
        if line.starts_with("---") && !fence_info_string(line).is_empty() {
            open_fence = Some(ln);
            continue;
        }

        // A single leading `---` is harmless: it opens the one document this
        // file contains. What SYON forbids is a multi-document stream, so
        // only a marker after content has begun is an error.
        if t == "---" || t.starts_with("--- ") || t.starts_with("---\t") {
            if seen_content {
                return Err(SyonError::forbidden(
                    ErrorCode::DocumentStartMarker,
                    format!("line {ln}: `---` starts a second document; SYON files hold exactly one"),
                ));
            }
            continue;
        }
        if !t.is_empty() && !t.starts_with('#') {
            seen_content = true;
        }
        if t == "..." || t.starts_with("... ") {
            return Err(SyonError::forbidden(
                ErrorCode::DocumentEndMarker,
                format!("line {ln}: `...` closes a document fence, but none is open"),
            ));
        }
        if t == "?" || t.starts_with("? ") {
            return Err(SyonError::forbidden(
                ErrorCode::ComplexKey,
                format!("line {ln}: complex key `?` is not allowed in SYON"),
            ));
        }

        // `[[[ ... ]]]` was SYON's own literal escape hatch. It is gone: a
        // `|` block scalar does the same job in syntax a YAML 1.2 parser
        // already understands. Name it rather than letting `[` fall through
        // to the generic flow-collection error, which would not say what to
        // write instead.
        if t.trim_end() == "[[[" || t.trim_end() == "]]]" {
            return Err(SyonError::forbidden(
                ErrorCode::LiteralBlockRemoved,
                format!(
                    "line {ln}: `[[[ ... ]]]` literal blocks were removed; \
                     use a `|` block scalar instead"
                ),
            ));
        }

        // Indicator characters are NOT scanned here.
        //
        // `&`, `*`, `!` and the flow openers are YAML indicators only at the
        // start of a node. Inside scalar content they are ordinary bytes --
        // `2>&1`, `Hello, World!` and `echo a && echo b` contain no anchor,
        // no tag and no flow collection. Whether a byte sits inside scalar
        // content is decided by parsing, so a pre-parse scan cannot answer
        // the question at all; it can only guess, and it guessed wrong on
        // every shell redirect.
        //
        // These constructs are rejected in `check_node_start` instead, which
        // runs once the grammar has established where each node begins.
    }

    Ok(())
}

/// Reject SYON's forbidden constructs at a position where they are genuinely
/// indicators: the start of a node's value.
///
/// `text` is a scalar exactly as written, with the `: ` or `- ` operator and
/// surrounding space already stripped.
fn check_node_start(text: &str) -> Result<(), SyonError> {
    let t = text.trim_start();
    let mut cs = t.chars();
    let (Some(first), second) = (cs.next(), cs.next()) else { return Ok(()) };

    let named = second.is_some_and(|c| c.is_alphanumeric() || c == '_');
    match first {
        '&' if named => Err(SyonError::forbidden(
            ErrorCode::Anchor,
            "anchor `&name` is not allowed in SYON",
        )),
        '*' if named => Err(SyonError::forbidden(
            ErrorCode::Alias,
            "alias `*name` is not allowed in SYON",
        )),
        '!' if named || second == Some('!') => Err(SyonError::forbidden(
            ErrorCode::ExplicitTag,
            "tag `!` / `!!` is not allowed in SYON",
        )),
        // `[` and `{` are deliberately absent.
        //
        // They are neither interpreted nor rejected here: the text is carried
        // through as an ordinary scalar and handed to the consumer, which
        // decides what it means. That is what "safe" denotes in SYON -- the
        // generic parser declines to interpret, rather than refusing the
        // input. `[a, b]` reaches the application as the string "[a, b]",
        // and `{{ .TASK }}` as the string "{{ .TASK }}".
        //
        // Anchors, aliases and tags above are different in kind: they are
        // reference and typing machinery *of the parser itself*, so passing
        // them through would mean interpreting them. They stay rejected.
        _ => Ok(()),
    }
}

/// Check a structural line's indentation and return its nesting depth.
fn depth_of(indent: usize, opts: &ParseOptions) -> Result<usize, SyonError> {
    // A step of 0 disables the check, for callers that want ragged input.
    let Some(depth) = indent.checked_div(opts.space_count) else {
        return Ok(indent);
    };
    if indent.is_multiple_of(opts.space_count) {
        Ok(depth)
    } else {
        Err(SyonError::syntax(
            ErrorCode::IndentNotMultipleOfStep,
            format!(
                "indented {indent} spaces, which is not a multiple of {} -- \
                 SYON uses a fixed indentation step",
                opts.space_count
            ),
        ))
    }
}

/// Strip one layer of quoting from a scalar written inline.
///
/// Mirrors what `extract_scalar` does for an ordinary mapping entry: double
/// quotes take backslash escapes, single quotes are literal with `''` for one
/// apostrophe.
fn unquote_scalar(text: &str) -> String {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"' {
        return unescape_dq(&text[1..text.len() - 1]);
    }
    if bytes.len() >= 2 && bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'' {
        return text[1..text.len() - 1].replace("''", "'");
    }
    text.to_string()
}

/// Split `key: value` out of a sequence item's inline scalar.
///
/// `- task: build` is a compact block mapping: one entry, written on the same
/// line as the dash. The grammar is line-oriented and hands the whole line
/// over as a scalar, so the entry is recovered here.
///
/// The split happens at the first `:` that is followed by a space or ends the
/// line, and that is not inside quotes -- the same spacing rule the grammar
/// applies. `- echo done: ok` therefore becomes a mapping, exactly as YAML
/// reads it; a command that must stay a command quotes the colon.
fn split_compact_entry(text: &str) -> Option<(String, String)> {
    let (mut in_sq, mut in_dq, mut esc) = (false, false, false);

    for (i, ch) in text.char_indices() {
        if esc {
            esc = false;
            continue;
        }
        match ch {
            '\\' if in_dq => esc = true,
            '"' if !in_sq => in_dq = !in_dq,
            '\'' if !in_dq => in_sq = !in_sq,
            ':' if !in_sq && !in_dq => {
                let rest = &text[i + 1..];
                if rest.is_empty() || rest.starts_with(' ') {
                    let key = text[..i].trim();
                    if key.is_empty() || key.starts_with([':', '-', '#']) {
                        return None;
                    }
                    // Unquote the value, as the ordinary `key: value` path
                    // does. Leaving the quotes on turns `- sh: '[ x = y ]'`
                    // into a single quoted word the shell cannot run.
                    return Some((key.to_string(), unquote_scalar(rest.trim())));
                }
            }
            _ => {}
        }
    }
    None
}

/// Prefix an error with the source line it came from.
fn at_line(e: SyonError, line: usize) -> SyonError {
    match e {
        SyonError::Forbidden { code, message } => {
            SyonError::forbidden(code, format!("line {line}: {message}"))
        }
        SyonError::Syntax { code, message } => {
            SyonError::syntax(code, format!("line {line}: {message}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Flat line representation after pest tokenisation
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum Line {
    Comment { indent: usize, text: String },
    KeyValue { indent: usize, key: String, value: Option<LineValue>, trailing: Option<String> },
    ListItem { indent: usize, value: Option<LineValue>, trailing: Option<String> },
    FenceOpen { path: String, format: String },
    FenceClose,
    /// A line matching no structural rule. Valid only as the body of a block
    /// scalar; anywhere else the Builder reports it as a syntax error.
    Raw { indent: usize, text: String, line: usize },
}

#[derive(Debug)]
enum LineValue {
    Scalar(String),
    /// A `|` (or `>`) header. The content is the following more-indented
    /// lines, which the Builder gathers.
    BlockHeader(Chomp),
}

/// Trailing-newline handling for a block scalar, from the `-` / `+` suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chomp {
    /// `|` -- one trailing newline.
    Clip,
    /// `|-` -- no trailing newline.
    Strip,
    /// `|+` -- keep every trailing newline.
    Keep,
}

/// Recognise a block scalar header.
///
/// SYON has no folded style: `>` is accepted as a spelling of `|` so that
/// YAML written for other tools keeps its meaning, rather than silently
/// folding newlines into spaces.
fn block_header(text: &str) -> Option<Chomp> {
    let t = text.trim_end();
    let rest = t.strip_prefix('|').or_else(|| t.strip_prefix('>'))?;
    match rest {
        "" => Some(Chomp::Clip),
        "-" => Some(Chomp::Strip),
        "+" => Some(Chomp::Keep),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Turn the flat pest output into Line structs
// ---------------------------------------------------------------------------

fn collect_lines(
    input: &str,
    opts: &ParseOptions,
) -> Result<(Vec<Line>, Vec<usize>), SyonError> {
    let pairs = SyonParser::parse(Rule::document, input).map_err(|e| {
        SyonError::syntax(ErrorCode::MalformedStructure, format!("{e}"))
    })?;

    let mut lines = Vec::new();
    // Source line of each entry in `lines`, so a block scalar can be taken
    // verbatim from the original text rather than from tokens.
    let mut line_nos: Vec<usize> = Vec::new();

    for pair in pairs.into_iter().next().unwrap().into_inner() {
        match pair.as_rule() {
            Rule::comment => {
                let line_no = pair.line_col().0;
                let mut indent = 0usize;
                let mut text = String::new();
                for inner in pair.into_inner() {
                    match inner.as_rule() {
                        Rule::indent => indent = inner.as_str().len(),
                        Rule::comment_text => text = inner.as_str().to_owned(),
                        _ => {}
                    }
                }
                lines.push(Line::Comment { indent, text });
                line_nos.push(line_no);
            }

            Rule::mapping_entry => {
                let line_no = pair.line_col().0;
                let mut indent = 0usize;
                let mut key = String::new();
                let mut value: Option<LineValue> = None;
                let mut trailing: Option<String> = None;
                for inner in pair.into_inner() {
                    match inner.as_rule() {
                        Rule::indent => indent = inner.as_str().len(),
                        Rule::key_body => key = inner.as_str().to_owned(),
                        Rule::inline_value => {
                            // Check the source as written. A quoted scalar is
                            // content by construction -- `"{{.TASK}}"` is a
                            // string, not a flow mapping -- and the quotes are
                            // gone by the time the value is extracted.
                            let quoted =
                                inner.as_str().trim_start().starts_with(['"', '\'']);
                            let v = parse_inline_value(inner);
                            if let (false, LineValue::Scalar(text)) = (quoted, &v) {
                                check_node_start(text)
                                    .map_err(|e| at_line(e, line_no))?;
                            }
                            value = Some(v);
                        }
                        Rule::trailing_comment => {
                            trailing = Some(extract_comment_text(inner));
                        }
                        _ => {}
                    }
                }
                // Validate key doesn't start with operator symbols
                let k = key.trim_start();
                if k.starts_with(':') || k.starts_with('-') || k.starts_with('#') {
                    return Err(SyonError::syntax(
                        ErrorCode::KeyStartsWithOperator,
                        format!("key {:?} must not start with an operator symbol", key),
                    ));
                }
                // A key also sits at node start, so `{a: b}` at line start
                // is a flow mapping rather than a key named `{a`.
                check_node_start(&key).map_err(|e| at_line(e, line_no))?;
                depth_of(indent, opts).map_err(|e| at_line(e, line_no))?;
                lines.push(Line::KeyValue { indent, key, value, trailing });
                line_nos.push(line_no);
            }

            Rule::sequence_item => {
                let line_no = pair.line_col().0;
                let mut indent = 0usize;
                let mut value: Option<LineValue> = None;
                let mut trailing: Option<String> = None;
                for inner in pair.into_inner() {
                    match inner.as_rule() {
                        Rule::indent => indent = inner.as_str().len(),
                        Rule::inline_value => {
                            // Check the source as written. A quoted scalar is
                            // content by construction -- `"{{.TASK}}"` is a
                            // string, not a flow mapping -- and the quotes are
                            // gone by the time the value is extracted.
                            let quoted =
                                inner.as_str().trim_start().starts_with(['"', '\'']);
                            let v = parse_inline_value(inner);
                            if let (false, LineValue::Scalar(text)) = (quoted, &v) {
                                check_node_start(text)
                                    .map_err(|e| at_line(e, line_no))?;
                            }
                            value = Some(v);
                        }
                        Rule::trailing_comment => {
                            trailing = Some(extract_comment_text(inner));
                        }
                        _ => {}
                    }
                }
                depth_of(indent, opts).map_err(|e| at_line(e, line_no))?;
                lines.push(Line::ListItem { indent, value, trailing });
                line_nos.push(line_no);
            }

            Rule::fence_open => {
                let line_no = pair.line_col().0;
                let mut path = String::new();
                let mut format = String::new();
                for inner in pair.into_inner() {
                    match inner.as_rule() {
                        Rule::fence_path => path = inner.as_str().to_owned(),
                        Rule::fence_format => format = inner.as_str().to_owned(),
                        _ => {}
                    }
                }
                lines.push(Line::FenceOpen { path, format });
                line_nos.push(line_no);
            }

            Rule::fence_close => {
                let line_no = pair.line_col().0;
                lines.push(Line::FenceClose);
                line_nos.push(line_no);
            }

            Rule::raw_line => {
                let line = pair.line_col().0;
                let line_no = line;
                // The document marker is structure, not content. `preflight`
                // has already rejected a second one, so this opens the single
                // document and carries nothing to build.
                if pair.as_str().trim() == "---" {
                    continue;
                }
                let text = pair.as_str().trim_end_matches(['\n', '\r']).to_owned();
                let indent = text.len() - text.trim_start().len();
                lines.push(Line::Raw { indent, text, line });
                line_nos.push(line_no);
            }

            Rule::EOI => {}
            _ => {}
        }
    }

    Ok((lines, line_nos))
}

fn parse_inline_value(pair: Pair<Rule>) -> LineValue {
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::scalar_value {
            let s = extract_scalar(inner);
            return match block_header(&s) {
                Some(chomp) => LineValue::BlockHeader(chomp),
                None => LineValue::Scalar(s),
            };
        }
    }
    LineValue::Scalar(String::new())
}

fn extract_scalar(pair: Pair<Rule>) -> String {
    for inner in pair.into_inner() {
        match inner.as_rule() {
            Rule::dq_scalar => {
                let s = inner.as_str();
                // Strip surrounding quotes and unescape
                return unescape_dq(&s[1..s.len() - 1]);
            }
            Rule::sq_scalar => {
                let s = inner.as_str();
                // Single quotes are literal throughout; `''` is one quote.
                return s[1..s.len() - 1].replace("''", "'");
            }
            Rule::plain_scalar => {
                // Both ends: the `: ` operator consumes one space, so
                // `key:   value` would otherwise carry the rest into the
                // value. A plain scalar cannot hold leading or trailing
                // spaces in YAML either -- quote it to keep them.
                return inner.as_str().trim().to_owned();
            }
            _ => {}
        }
    }
    String::new()
}

fn unescape_dq(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('"') => out.push('"'),
                Some('\\') => out.push('\\'),
                Some(c) => { out.push('\\'); out.push(c); }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn extract_comment_text(pair: Pair<Rule>) -> String {
    for inner in pair.into_inner() {
        if inner.as_rule() == Rule::inline_comment_text {
            return inner.as_str().trim_end().to_owned();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Build AST from flat Line list using indentation stack
// ---------------------------------------------------------------------------

struct Builder<'a> {
    opts: ParseOptions,
    lines: &'a [Line],
    /// Source line of each entry in `lines`, 1-based.
    line_nos: &'a [usize],
    /// The original text, split into lines. A block scalar is verbatim, so
    /// its body has to come from here rather than from tokens -- tokenising
    /// `# 7 solutions` inside a `|` block would turn content into a comment.
    src: &'a [&'a str],
    pos: usize,
}

impl<'a> Builder<'a> {
    fn new(
        opts: ParseOptions,
        lines: &'a [Line],
        line_nos: &'a [usize],
        src: &'a [&'a str],
    ) -> Self {
        Self { opts, lines, line_nos, src, pos: 0 }
    }

    fn peek_indent(&self) -> Option<usize> {
        self.lines.get(self.pos).map(|l| match l {
            Line::Comment { indent, .. } => *indent,
            Line::KeyValue { indent, .. } => *indent,
            Line::ListItem { indent, .. } => *indent,
            _ => 0,
        })
    }

    /// Gather the body of a `|` block scalar, verbatim from the source.
    ///
    /// Content is every following line indented past `owner_indent`, minus the
    /// block's common indentation. It is read from the original text because
    /// a block body is not SYON: `# 7 solutions` inside one is a shell
    /// comment, and the grammar would otherwise tokenise it as a SYON comment
    /// and end the block early.
    fn take_block_scalar(
        &mut self,
        chomp: Chomp,
        owner_indent: usize,
        header_line: usize,
    ) -> String {
        let mut body: Vec<&str> = Vec::new();
        let mut last = header_line;
        let mut i = header_line; // src[header_line] is the line after the header

        while let Some(line) = self.src.get(i) {
            if line.trim().is_empty() {
                body.push(line);
                i += 1;
                continue;
            }
            if line.len() - line.trim_start().len() > owner_indent {
                body.push(line);
                i += 1;
                last = i;
            } else {
                break;
            }
        }
        // Trailing blank lines belong to whatever follows, not to the block.
        while body.last().is_some_and(|l| l.trim().is_empty()) {
            body.pop();
        }

        let indent = body
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);

        let mut out = String::new();
        for l in &body {
            out.push_str(l.get(indent..).unwrap_or(""));
            out.push('\n');
        }

        // Skip every token produced from the lines just consumed.
        while self.line_nos.get(self.pos).is_some_and(|&n| n <= last) {
            self.pos += 1;
        }

        match chomp {
            Chomp::Strip => while out.ends_with('\n') { out.pop(); },
            Chomp::Clip => while out.ends_with("\n\n") { out.pop(); },
            Chomp::Keep => {}
        }
        out
    }

    /// Column at which a compact entry's key starts, for `- key: ...`.
    ///
    /// The owner indent of a compact entry's block scalar is the key's column,
    /// not the list item's: a sibling entry of the same compact mapping aligns
    /// under the key, and must end the block rather than be swallowed by it.
    /// Read from the source rather than assumed to be `indent + 2`, since the
    /// padding after `-` is trimmed before the scalar reaches us.
    fn compact_key_column(&self, header_line: usize, indent: usize) -> usize {
        let Some(line) = self.src.get(header_line.wrapping_sub(1)) else {
            return indent + 2;
        };
        // Past the indent and the `-`, then past whatever padding follows it.
        let Some(after_dash) = line.get(indent + 1..) else {
            return indent + 2;
        };
        indent + 1 + (after_dash.len() - after_dash.trim_start().len())
    }

    /// Fold continuation lines into the preceding plain scalar.
    ///
    /// A continuation carries no marker; it is identified by being indented
    /// past its owner and by not being structural. Only `Raw` lines qualify,
    /// so a genuine child block still nests:
    ///
    /// ```text
    /// - task: build      <- ListItem
    ///   vars:            <- KeyValue, a child mapping, not a continuation
    /// ```
    ///
    /// Newlines fold to single spaces, as in YAML.
    fn take_continuation(&mut self, owner_indent: usize) -> Option<String> {
        let mut parts: Vec<&str> = Vec::new();
        while let Some(Line::Raw { indent, text, .. }) = self.lines.get(self.pos) {
            if *indent <= owner_indent {
                break;
            }
            parts.push(text.trim());
            self.pos += 1;
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    fn peek_is_fence(&self) -> bool {
        matches!(self.lines.get(self.pos), Some(Line::FenceOpen { .. }) | Some(Line::FenceClose))
    }

    /// Collect pending comment lines at or above `min_indent`.
    fn collect_comments(&mut self, min_indent: usize) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(Line::Comment { indent, text }) = self.lines.get(self.pos) {
            if *indent < min_indent {
                break;
            }
            out.push(text.clone());
            self.pos += 1;
        }
        out
    }

    /// Parse a block of lines all sharing `expected_indent`, returning a Value.
    /// Returns None if there are no applicable lines at this indent.
    fn parse_block(&mut self, expected_indent: usize) -> Result<Option<Value>, SyonError> {
        // Peek at the first real (non-comment) line
        let save = self.pos;
        // Skip comments temporarily to see what kind of block follows
        let mut scan = self.pos;
        while let Some(Line::Comment { .. }) = self.lines.get(scan) {
            scan += 1;
        }
        match self.lines.get(scan) {
            None | Some(Line::FenceOpen { .. }) | Some(Line::FenceClose) => Ok(None),
            Some(Line::KeyValue { indent, .. }) if *indent == expected_indent => {
                Ok(Some(self.parse_mapping(expected_indent)?))
            }
            Some(Line::ListItem { indent, .. }) if *indent == expected_indent => {
                Ok(Some(self.parse_sequence(expected_indent)?))
            }
            Some(Line::KeyValue { indent, .. }) | Some(Line::ListItem { indent, .. })
                if *indent != expected_indent =>
            {
                // Different indent level — not our block
                let _ = save;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn parse_mapping(&mut self, indent: usize) -> Result<Value, SyonError> {
        let mut entries: Vec<MappingEntry> = Vec::new();

        loop {
            let leading_comments = self.collect_comments(indent);

            match self.lines.get(self.pos) {
                Some(Line::KeyValue { indent: kv_indent, .. }) if *kv_indent == indent => {}
                _ => {
                    // Put comment lines back? No — they're consumed. But if no
                    // key follows, these are "trailing" block comments we discard
                    // from the mapping (they'd belong to a parent).
                    // Re-wind comment consumption if nothing followed:
                    if !leading_comments.is_empty() {
                        // Back up: rewind past the consumed comments
                        self.pos -= leading_comments.len();
                    }
                    break;
                }
            }

            if let Some(Line::KeyValue { key, value, trailing, indent: _ }) =
                self.lines.get(self.pos)
            {
                let key = key.clone();
                let entry_line = self.line_nos.get(self.pos).copied().unwrap_or(0);
                let header = match value.as_ref() {
                    Some(LineValue::BlockHeader(c)) => Some(*c),
                    _ => None,
                };
                let inline_val = value.as_ref().and_then(|v| match v {
                    LineValue::Scalar(s) => Some(Value::Scalar(s.clone())),
                    LineValue::BlockHeader(_) => None,
                });
                let trailing_comment = trailing.clone();
                let header_line = self.line_nos.get(self.pos).copied().unwrap_or(0);
                self.pos += 1;
                let inline_val = match header {
                    Some(c) => {
                        Some(Value::LiteralBlock(self.take_block_scalar(c, indent, header_line)))
                    }
                    None => inline_val,
                };
                // A plain scalar may continue on the following deeper lines.
                let inline_val = match inline_val {
                    Some(Value::Scalar(s)) if header.is_none() => Some(Value::Scalar(
                        match self.take_continuation(indent) {
                            Some(rest) => format!("{s} {rest}"),
                            None => s,
                        },
                    )),
                    other => other,
                };

                // Check for a child block at indent+1 (or more)
                let child_indent = self.peek_indent();
                let child_value = if let Some(ci) = child_indent {
                    if ci > indent && !self.peek_is_fence() {
                        self.parse_block(ci)?
                    } else {
                        None
                    }
                } else {
                    None
                };

                let value = match (inline_val, child_value) {
                    (_, Some(child)) => child,
                    (Some(iv), None) => iv,
                    (None, None) => Value::Scalar(String::new()),
                };

                // Duplicate key check
                if entries.iter().any(|e| e.key == key) {
                    return Err(SyonError::syntax(
                        ErrorCode::DuplicateKey,
                        format!("duplicate key {:?}", key),
                    ));
                }

                entries.push(MappingEntry {
                    line: entry_line,
                    key,
                    value,
                    leading_comments,
                    trailing_comment,
                });
            }
        }

        Ok(Value::Mapping(entries))
    }

    fn parse_sequence(&mut self, indent: usize) -> Result<Value, SyonError> {
        let mut items: Vec<SequenceItem> = Vec::new();

        loop {
            let leading_comments = self.collect_comments(indent);

            match self.lines.get(self.pos) {
                Some(Line::ListItem { indent: li_indent, .. }) if *li_indent == indent => {}
                _ => {
                    if !leading_comments.is_empty() {
                        self.pos -= leading_comments.len();
                    }
                    break;
                }
            }

            if let Some(Line::ListItem { value, trailing, indent: _ }) = self.lines.get(self.pos) {
                let item_line = self.line_nos.get(self.pos).copied().unwrap_or(0);
                let header = match value.as_ref() {
                    Some(LineValue::BlockHeader(c)) => Some(*c),
                    _ => None,
                };
                let inline_val = value.as_ref().and_then(|v| match v {
                    LineValue::Scalar(s) => Some(Value::Scalar(s.clone())),
                    LineValue::BlockHeader(_) => None,
                });
                let trailing_comment = trailing.clone();
                let header_line = self.line_nos.get(self.pos).copied().unwrap_or(0);

                // `- key: |` -- a compact entry whose value is a block scalar.
                //
                // This is decided here, before anything below consumes the
                // following lines. `take_block_scalar` reads raw source, since
                // a block scalar's body is verbatim text that must never be
                // parsed as structure; by the time the compact-mapping arm
                // further down runs, `take_continuation` has folded those
                // lines into the scalar and `parse_block` has reshaped them
                // into a Value. Both are unrecoverable, and neither reports an
                // error -- `- md: |` used to yield the bare string "|".
                let compact_block = match value.as_ref() {
                    Some(LineValue::Scalar(sc)) => split_compact_entry(sc)
                        .and_then(|(k, v)| block_header(&v).map(|chomp| (k, chomp))),
                    _ => None,
                };
                // Detected whether or not the option is on. With it off the
                // body would otherwise be folded into the scalar by
                // `take_continuation` and the `|` left as literal text -- a
                // silent wrong answer, which is the defect this whole path
                // exists to remove. Say so instead.
                if compact_block.is_some() && !self.opts.allow_key_in_line_after_list {
                    return Err(SyonError::syntax(
                        ErrorCode::CompactBlockScalarNeedsOption,
                        format!(
                            "line {header_line}: a sequence item's `key: |` block scalar needs \
                             `allow_key_in_line_after_list`; without it, write the `-` on its \
                             own line and the key beneath it"
                        ),
                    ));
                }
                if let Some((key, chomp)) = compact_block {
                    let owner = self.compact_key_column(header_line, indent);
                    self.pos += 1;
                    let body = self.take_block_scalar(chomp, owner, header_line);
                    let mut entries = vec![MappingEntry {
                        line: item_line,
                        key,
                        value: Value::LiteralBlock(body),
                        leading_comments: Vec::new(),
                        trailing_comment: None,
                    }];
                    // Remaining entries of the same compact mapping, e.g. the
                    // `vars:` block under `- task: build`.
                    if let Some(ci) = self.peek_indent() {
                        if ci > indent && !self.peek_is_fence() {
                            match self.parse_block(ci)? {
                                Some(Value::Mapping(rest)) => entries.extend(rest),
                                Some(_) => {
                                    return Err(SyonError::syntax(
                                        ErrorCode::SequenceItemMixesMappingAndBlock,
                                        "a sequence item mixes `key: value` with a \
                                         non-mapping block",
                                    ))
                                }
                                None => {}
                            }
                        }
                    }
                    items.push(SequenceItem {
                        line: item_line,
                        value: Value::Mapping(entries),
                        leading_comments,
                        trailing_comment,
                    });
                    continue;
                }

                self.pos += 1;
                let inline_val = match header {
                    Some(c) => {
                        Some(Value::LiteralBlock(self.take_block_scalar(c, indent, header_line)))
                    }
                    None => inline_val,
                };
                // A plain scalar may continue on the following deeper lines.
                let inline_val = match inline_val {
                    Some(Value::Scalar(s)) if header.is_none() => Some(Value::Scalar(
                        match self.take_continuation(indent) {
                            Some(rest) => format!("{s} {rest}"),
                            None => s,
                        },
                    )),
                    other => other,
                };

                // `- key:` with nothing after the colon, e.g.
                //
                //     - tabs:
                //         - viewer
                //       side: right
                //
                // Lines deeper than the key's column are the key's value;
                // lines aligned with the key are further entries of the same
                // compact mapping -- the nesting YAML reads, and what the
                // expanded form (`-` alone, `tabs:` beneath it) always gave.
                // Parsing the whole deeper block as one child instead made
                // the nested sequence look like a non-mapping sibling
                // (SYON-132), and silently flattened a nested mapping into
                // the item.
                let empty_compact_key = match &inline_val {
                    Some(Value::Scalar(sc)) if self.opts.allow_key_in_line_after_list => {
                        split_compact_entry(sc).filter(|(_, v)| v.is_empty()).map(|(k, _)| k)
                    }
                    _ => None,
                };
                if let Some(key) = empty_compact_key {
                    let key_column = self.compact_key_column(header_line, indent);
                    let mut value = Value::Scalar(String::new());
                    if let Some(ci) = self.peek_indent() {
                        if ci > key_column && !self.peek_is_fence() {
                            if let Some(nested) = self.parse_block(ci)? {
                                value = nested;
                            }
                        }
                    }
                    let mut entries = vec![MappingEntry {
                        line: item_line,
                        key,
                        value,
                        leading_comments: Vec::new(),
                        trailing_comment: None,
                    }];
                    if let Some(ci) = self.peek_indent() {
                        if ci > indent && !self.peek_is_fence() {
                            match self.parse_block(ci)? {
                                Some(Value::Mapping(rest)) => entries.extend(rest),
                                Some(_) => {
                                    return Err(SyonError::syntax(
                                        ErrorCode::SequenceItemMixesMappingAndBlock,
                                        "a sequence item mixes `key: value` with a \
                                         non-mapping block",
                                    ))
                                }
                                None => {}
                            }
                        }
                    }
                    items.push(SequenceItem {
                        line: item_line,
                        value: Value::Mapping(entries),
                        leading_comments,
                        trailing_comment,
                    });
                    continue;
                }

                let child_indent = self.peek_indent();
                let child_value = if let Some(ci) = child_indent {
                    if ci > indent && !self.peek_is_fence() {
                        self.parse_block(ci)?
                    } else {
                        None
                    }
                } else {
                    None
                };

                let value = match (inline_val, child_value) {
                    // `- key: value`, optionally followed by more entries on
                    // deeper lines, is one compact mapping. Both halves are
                    // kept: dropping the inline half loses `task:` from
                    // `- task: build` / `  vars: ...` with no error at all.
                    (Some(Value::Scalar(ref sc)), child)
                        if self.opts.allow_key_in_line_after_list
                            && split_compact_entry(sc).is_some() =>
                    {
                        let (key, val) = split_compact_entry(sc).unwrap();
                        let mut entries = vec![MappingEntry {
                            line: item_line,
                            key,
                            value: Value::Scalar(val),
                            leading_comments: Vec::new(),
                            trailing_comment: None,
                        }];
                        match child {
                            Some(Value::Mapping(rest)) => entries.extend(rest),
                            Some(_) => {
                                return Err(SyonError::syntax(
                                    ErrorCode::SequenceItemMixesMappingAndBlock,
                                    "a sequence item mixes `key: value` with a non-mapping block",
                                ))
                            }
                            None => {}
                        }
                        Value::Mapping(entries)
                    }
                    // Both halves present but not merged: say so, rather than
                    // dropping one of them silently.
                    (Some(Value::Scalar(sc)), Some(_)) if !sc.trim().is_empty() => {
                        return Err(SyonError::syntax(
                            ErrorCode::SequenceItemInlineTextAndBlock,
                            format!(
                                "sequence item has both inline text {sc:?} and an indented \
                                 block; enable `allow_key_in_line_after_list` to read it as \
                                 a compact mapping"
                            ),
                        ))
                    }
                    (_, Some(child)) => child,
                    (Some(iv), None) => iv,
                    (None, None) => Value::Scalar(String::new()),
                };

                items.push(SequenceItem {
                    line: item_line,
                    value,
                    leading_comments,
                    trailing_comment,
                });
            }
        }

        Ok(Value::Sequence(items))
    }

    /// Parse the top-level document body (indent 0 or first encountered indent).
    fn parse_document_body(&mut self) -> Result<Value, SyonError> {
        // Find the first non-comment indent without consuming comments — let
        // parse_mapping / parse_sequence collect them as leading_comments.
        let mut scan = self.pos;
        while let Some(Line::Comment { .. }) = self.lines.get(scan) {
            scan += 1;
        }

        let Some(first_indent) = (match self.lines.get(scan) {
            Some(Line::KeyValue { indent, .. }) | Some(Line::ListItem { indent, .. }) => {
                Some(*indent)
            }
            _ => None,
        }) else {
            return Ok(Value::Mapping(Vec::new()));
        };

        if self.peek_is_fence() {
            return Ok(Value::Mapping(Vec::new()));
        }

        let block = self.parse_block(first_indent)?;
        Ok(block.unwrap_or(Value::Mapping(Vec::new())))
    }

    /// Consume lines up to (but not including) the next FenceClose, building a Document.
    fn parse_fenced_document(
        &mut self,
        path: String,
        format: String,
    ) -> Result<Document, SyonError> {
        let body = self.parse_document_body()?;
        // Consume FenceClose if present
        if matches!(self.lines.get(self.pos), Some(Line::FenceClose)) {
            self.pos += 1;
        }
        Ok(Document { path: Some(path), format: Some(format), body })
    }
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Parse a SYON source string into a [`SyonFile`].
pub fn parse(input: &str) -> Result<SyonFile, SyonError> {
    parse_with(input, ParseOptions::default())
}

/// Parse under explicit options. See [`ParseOptions`].
pub fn parse_with(input: &str, opts: ParseOptions) -> Result<SyonFile, SyonError> {
    preflight(input)?;

    let (lines, line_nos) = collect_lines(input, &opts)?;
    let src: Vec<&str> = input.lines().collect();
    let mut builder = Builder::new(opts, &lines, &line_nos, &src);
    let mut documents: Vec<Document> = Vec::new();

    while builder.pos < builder.lines.len() {
        match builder.lines.get(builder.pos) {
            Some(Line::FenceOpen { path, format }) => {
                let path = path.clone();
                let format = format.clone();
                builder.pos += 1;
                let doc = builder.parse_fenced_document(path, format)?;
                documents.push(doc);
            }
            Some(Line::FenceClose) => {
                // Stray close — skip
                builder.pos += 1;
            }
            // A raw line still here was never claimed by a block scalar, so
            // it is genuinely malformed rather than verbatim content.
            Some(Line::Raw { text, line, .. }) => {
                return Err(SyonError::syntax(
                    ErrorCode::UnexpectedContent,
                    format!(
                        "line {line}: unexpected content {:?} -- expected a mapping entry, \
                         a sequence item, or the body of a `|` block",
                        text.trim()
                    ),
                ));
            }
            _ => {
                // Main (unfenced) document
                let before = builder.pos;
                let body = builder.parse_document_body()?;
                documents.push(Document { path: None, format: None, body });

                // Guarantee progress. `parse_document_body` gathers leading
                // comments and rewinds when no entry follows them, so a
                // document of only comments consumes nothing and would spin
                // here forever. Nothing consumed means nothing is left that
                // can be consumed.
                if builder.pos == before {
                    break;
                }
            }
        }
    }

    if documents.is_empty() {
        documents.push(Document { path: None, format: None, body: Value::Mapping(Vec::new()) });
    }

    Ok(SyonFile { documents })
}

/// Convenience: parse and return the first document's body.
pub fn parse_document(input: &str) -> Result<crate::ast::Document, SyonError> {
    let mut file = parse(input)?;
    Ok(file.documents.remove(0))
}

/// Parses the dialect the suite's data files are written in and returns the
/// main document's body.
///
/// Those files use the compact form for lists of mappings (`- id: p1` with the
/// rest of the mapping indented below), which the default options reject as
/// ambiguous with prose. Turning the option on here, in one place, keeps every
/// reader of such a file in agreement. An input with no main document (empty,
/// or only fenced documents) is an empty mapping.
pub fn parse_root(input: &str) -> Result<Value, SyonError> {
    let options = ParseOptions { allow_key_in_line_after_list: true, ..ParseOptions::default() };
    let file = parse_with(input, options)?;
    Ok(file.root().cloned().unwrap_or(Value::Mapping(Vec::new())))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Value;

    // --- Spacing rule: colon ---

    #[test]
    fn colon_space_is_key_separator() {
        let doc = parse_document("key: value\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].key, "key");
                assert_eq!(entries[0].value, Value::Scalar("value".into()));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn colon_without_space_is_literal() {
        // "https://example.com" — the `:` is not followed by a space so it's literal
        let doc = parse_document("url: https://example.com\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].key, "url");
                assert_eq!(entries[0].value, Value::Scalar("https://example.com".into()));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn only_first_colon_space_on_a_line_is_structural() {
        // Only the FIRST `: ` on a line separates key from value; every
        // later colon, even a `: `-shaped one, is ordinary value text.
        let doc = parse_document("key: value: with colon: multiple times\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].key, "key");
                assert_eq!(
                    entries[0].value,
                    Value::Scalar("value: with colon: multiple times".into())
                );
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    // --- Spacing rule: dash ---

    #[test]
    fn dash_space_is_list_item() {
        let doc = parse_document("- alpha\n- beta\n").unwrap();
        match &doc.body {
            Value::Sequence(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0].value, Value::Scalar("alpha".into()));
                assert_eq!(items[1].value, Value::Scalar("beta".into()));
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    #[test]
    fn dash_without_space_is_literal() {
        // "-draft" as a value should not become a list item
        let doc = parse_document("tag: -draft\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].value, Value::Scalar("-draft".into()));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn dash_is_structural_only_as_first_non_space_char_of_the_line() {
        // A `-` later in the line -- even followed by a space, even preceded
        // by a space -- is NOT a sequence-item marker unless it is the
        // first non-space character on the line.
        let doc = parse_document("note: this - is not a list item\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(
                    entries[0].value,
                    Value::Scalar("this - is not a list item".into())
                );
            }
            other => panic!("expected Mapping, got {other:?}"),
        }

        // A `-` inside a key (not preceded by whitespace at all) is also
        // just ordinary key text.
        let doc = parse_document("a-b: value\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].key, "a-b");
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    // --- Spacing rule: hash ---

    #[test]
    fn hash_space_is_comment() {
        // A comment-only document body should be empty mapping
        let doc = parse_document("# top comment\nkey: val\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].key, "key");
                // Comment is attached as leading comment on the entry
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn hash_without_space_is_literal_value() {
        // "abc#123" — `#` not preceded by space, so it's part of the value
        let doc = parse_document("id: abc#123\n").unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].value, Value::Scalar("abc#123".into()));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    // --- Literal block ---

    #[test]
    fn block_scalar_value_roundtrip() {
        // The `|` block scalar is now the only verbatim-text construct, and
        // carries what `key: [[[` used to in examples/glossary/*.syon.
        let input = "description: |\n  line one\n  line two\n";
        let doc = parse_document(input).unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries.len(), 1);
                match &entries[0].value {
                    Value::LiteralBlock(s) => {
                        assert!(s.contains("line one"), "got: {s:?}");
                        assert!(s.contains("line two"), "got: {s:?}");
                    }
                    other => panic!("expected LiteralBlock, got {other:?}"),
                }
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn bracketed_literal_block_is_rejected_by_name() {
        // Removing `[[[` silently would leave `[` to trip the generic
        // flow-collection error, which does not say what to write instead.
        for input in ["[[[\nline one\n]]]\n", "description: [[[\n  line one\n  ]]]\n"] {
            let err = parse_document(input).unwrap_err().to_string();
            assert!(err.contains("[[["), "got: {err}");
            assert!(err.contains("block scalar"), "got: {err}");
        }
    }

    // --- Block scalar bodies are verbatim ---

    #[test]
    fn markers_inside_a_block_scalar_body_are_content() {
        // Regression: preflight scanned every line for `---`, so three dashes
        // written inside a `|` body were rejected as a document marker even
        // though the body is verbatim text. syon-go always read it correctly.
        for marker in ["--", "---", "----", "...", "?", "[[[", "]]]"] {
            let input = format!("note: |\n  before\n  {marker}\n  after\n");
            let doc = parse_document(&input)
                .unwrap_or_else(|e| panic!("{marker:?} in a block body was rejected: {e}"));
            match doc.body {
                Value::Mapping(entries) => match &entries[0].value {
                    Value::LiteralBlock(body) => {
                        assert_eq!(body, &format!("before\n{marker}\nafter\n"));
                    }
                    other => panic!("expected LiteralBlock, got {other:?}"),
                },
                other => panic!("expected Mapping, got {other:?}"),
            }
        }
    }

    #[test]
    fn markers_outside_a_block_scalar_body_are_still_rejected() {
        // The exemption is scoped to the body. Dedenting back out restores it.
        for (input, want) in [
            ("a: 1\n---\nb: 2\n", ErrorCode::DocumentStartMarker),
            ("note: |\n  body\n---\n", ErrorCode::DocumentStartMarker),
            ("a: 1\n...\n", ErrorCode::DocumentEndMarker),
            ("? a\n", ErrorCode::ComplexKey),
        ] {
            assert_eq!(parse(input).unwrap_err().code(), want, "input {input:?}");
        }
    }

    #[test]
    fn a_scalar_ending_in_a_pipe_does_not_open_a_block_body() {
        // `cmd: echo a |` is a scalar, so the next line is structure again --
        // preflight must agree with `block_header`, which requires the token
        // to be the whole value.
        let err = parse("cmd: echo a |\n---\n").unwrap_err();
        assert_eq!(err.code(), ErrorCode::DocumentStartMarker);
    }

    // --- Error codes ---

    /// Every code this implementation can actually produce, pinned to the
    /// input that produces it. A code is API: renumbering one silently would
    /// break callers matching on it, so the table lives in a test rather than
    /// only in `spec/05-error-codes.md`.
    #[test]
    fn error_codes_are_stable() {
        let cases: &[(&str, ErrorCode)] = &[
            ("key:\n\t- item\n", ErrorCode::TabInIndentation),
            ("a: 1\n---\nb: 2\n", ErrorCode::DocumentStartMarker),
            ("a: 1\n...\n", ErrorCode::DocumentEndMarker),
            ("? a\n", ErrorCode::ComplexKey),
            ("desc: [[[\n  x\n  ]]]\n", ErrorCode::LiteralBlockRemoved),
            ("key: &anchor\n", ErrorCode::Anchor),
            ("key: *alias\n", ErrorCode::Alias),
            ("key: !!str x\n", ErrorCode::ExplicitTag),
            ("a: 1\na: 2\n", ErrorCode::DuplicateKey),
        ];
        for (input, want) in cases {
            let err = parse(input).unwrap_err();
            assert_eq!(err.code(), *want, "input {input:?} produced {err}");
        }
    }

    #[test]
    fn display_carries_the_code() {
        let err = parse("a: 1\n...\n").unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("[SYON-118]"), "got {text}");
        assert!(text.contains("closes a document fence"), "got {text}");
    }

    // --- Forbidden constructs ---

    #[test]
    fn reject_yaml_tag() {
        let err = parse_document("key: !!str value\n").unwrap_err().to_string();
        assert!(err.contains("tag") || err.contains("!"), "got: {err}");
    }

    #[test]
    fn reject_anchor() {
        let err = parse_document("key: &anchor value\n").unwrap_err().to_string();
        assert!(err.contains("anchor") || err.contains("&"), "got: {err}");
    }

    #[test]
    fn reject_alias() {
        let err = parse_document("a: &anc val\nb: *anc\n").unwrap_err().to_string();
        assert!(err.contains("alias") || err.contains("anchor") || err.contains("*"), "got: {err}");
    }

    #[test]
    fn flow_sequence_passes_through_as_text() {
        // SYON does not interpret flow syntax, and does not reject it either.
        // The value reaches the consumer verbatim, to interpret or not.
        let doc = parse_document("key: [a, b]\n").unwrap();
        let Value::Mapping(entries) = doc.body else { panic!("expected a mapping") };
        assert_eq!(entries[0].value, Value::Scalar("[a, b]".into()));
    }

    #[test]
    fn flow_mapping_passes_through_as_text() {
        let doc = parse_document("key: {a: 1}\n").unwrap();
        let Value::Mapping(entries) = doc.body else { panic!("expected a mapping") };
        assert_eq!(entries[0].value, Value::Scalar("{a: 1}".into()));
    }

    #[test]
    fn template_expressions_pass_through_as_text() {
        let doc = parse_document("key: {{ .TASK }}-suffix\n").unwrap();
        let Value::Mapping(entries) = doc.body else { panic!("expected a mapping") };
        assert_eq!(entries[0].value, Value::Scalar("{{ .TASK }}-suffix".into()));
    }

    #[test]
    fn reject_second_document_start() {
        // A leading `---` opens the single document this file holds, and is
        // accepted. What SYON forbids is a multi-document stream.
        assert!(parse_document("---\nkey: value\n").is_ok());
        assert!(parse_document("key: value\n---\nother: value\n").is_err());
    }

    #[test]
    fn reject_complex_key() {
        assert!(parse_document("? complex key\n: value\n").is_err());
    }

    // --- Nested mapping ---

    #[test]
    fn nested_mapping_parses() {
        let input = "outer:\n  inner: value\n";
        let doc = parse_document(input).unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].key, "outer");
                match &entries[0].value {
                    Value::Mapping(inner) => {
                        assert_eq!(inner[0].key, "inner");
                        assert_eq!(inner[0].value, Value::Scalar("value".into()));
                    }
                    other => panic!("expected inner Mapping, got {other:?}"),
                }
            }
            other => panic!("expected outer Mapping, got {other:?}"),
        }
    }

    // --- Multi-document fence ---

    #[test]
    fn multi_document_fence_separates_documents() {
        let input = "---config.json\nkey: value\n...\n";
        let file = parse(input).unwrap();
        let fenced = file.documents.iter().find(|d| d.path.is_some()).unwrap();
        assert_eq!(fenced.path.as_deref(), Some("config"));
        assert_eq!(fenced.format.as_deref(), Some("json"));
    }

    #[test]
    fn a_fence_may_run_to_end_of_file_without_a_close() {
        // `...` is optional, exactly as YAML's document-end marker is: a fence
        // that reaches the end of the file is complete, not truncated. This
        // replaced an earlier rule that rejected an unclosed fence outright.
        let file = parse("---path.json\nkey: value\n").unwrap();
        let fenced = file.documents.iter().find(|d| d.path.is_some()).unwrap();
        assert_eq!(fenced.path.as_deref(), Some("path"));
        assert_eq!(fenced.format.as_deref(), Some("json"));
    }

    #[test]
    fn paired_fences_each_keep_their_own_path() {
        // Two fenced documents in a row: the close of the first must not be
        // read as the open of the second.
        let input = "---a.json\nkey: value\n...\n---b.json\nother: thing\n...\n";
        let file = parse(input).unwrap();
        let paths: Vec<_> = file.documents.iter().filter_map(|d| d.path.as_deref()).collect();
        assert_eq!(paths, vec!["a", "b"]);
    }

    #[test]
    fn backticks_inside_a_block_scalar_are_content_not_a_fence() {
        // The body of a `|` block is indented under its header, so a ``` line
        // there is never at column 0 and must not open a fence.
        let input = "readme: |\n  ```sh\n  echo hi\n  ```\n";
        let file = parse(input).unwrap();
        assert_eq!(file.documents.len(), 1);
        assert!(file.documents[0].path.is_none());
    }

    // --- Duplicate key rejection ---

    #[test]
    fn reject_duplicate_keys() {
        let err = parse_document("a: 1\na: 2\n").unwrap_err().to_string();
        assert!(err.contains("duplicate"), "got: {err}");
    }

    #[test]
    fn sequence_item_with_embedded_colon_is_not_mistaken_for_a_key() {
        // A list item's plain-scalar text containing "word: " (e.g. ordinary
        // prose) must not be misparsed as a mapping_entry -- the leading
        // "- " would otherwise get swept into a bogus key and hard-reject,
        // instead of falling back to sequence_item. See grammar.pest's
        // key_body comment for the mechanism.
        let input = "- Note: see the appendix for details\n- second item\n";
        let doc = parse_document(input).unwrap();
        match &doc.body {
            Value::Sequence(items) => {
                assert_eq!(items.len(), 2);
                match &items[0].value {
                    Value::Scalar(s) => assert_eq!(s, "Note: see the appendix for details"),
                    other => panic!("expected Scalar, got {other:?}"),
                }
            }
            other => panic!("expected Sequence, got {other:?}"),
        }
    }

    // --- Comment attachment ---

    #[test]
    fn leading_comment_attached_to_entry() {
        let input = "# section header\nkey: value\n";
        let doc = parse_document(input).unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert!(!entries[0].leading_comments.is_empty(),
                    "expected leading comments on entry");
                assert_eq!(entries[0].leading_comments[0], "section header");
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    #[test]
    fn trailing_comment_attached_to_entry() {
        let input = "key: value # side note\n";
        let doc = parse_document(input).unwrap();
        match &doc.body {
            Value::Mapping(entries) => {
                assert_eq!(entries[0].trailing_comment.as_deref(), Some("side note"));
            }
            other => panic!("expected Mapping, got {other:?}"),
        }
    }

    // --- Taskfile YAML compatibility -------------------------------------

    fn scalar_of(src: &str, key: &str) -> String {
        let doc = parse_document(src).unwrap();
        let Value::Mapping(entries) = doc.body else { panic!("expected a mapping") };
        let e = entries.into_iter().find(|e| e.key == key).expect("key not found");
        match e.value {
            Value::Scalar(s) | Value::LiteralBlock(s) => s,
            other => panic!("expected a scalar, got {other:?}"),
        }
    }

    #[test]
    fn indicator_chars_inside_scalars_are_content() {
        // `&` and `!` are indicators only at node start. A shell redirect is
        // not an anchor.
        assert_eq!(scalar_of("a: command -v curl >/dev/null 2>&1\n", "a"),
                   "command -v curl >/dev/null 2>&1");
        assert_eq!(scalar_of("a: Hello, World!\n", "a"), "Hello, World!");
        assert_eq!(scalar_of("a: echo x && echo y\n", "a"), "echo x && echo y");
    }

    #[test]
    fn anchors_and_tags_are_still_rejected_at_node_start() {
        assert!(parse_document("a: &anc val\n").is_err());
        assert!(parse_document("a: !!str 3\n").is_err());
    }

    #[test]
    fn block_scalar_keeps_its_body_verbatim() {
        // The body is not SYON: `# 7 solutions` is shell, not a comment, and
        // must not end the block.
        let src = "a: |\n  cat <<'EOF'\n  #----\n  # 7 solutions\n  alias do=\"task\"\n  EOF\n";
        assert_eq!(
            scalar_of(src, "a"),
            "cat <<'EOF'\n#----\n# 7 solutions\nalias do=\"task\"\nEOF\n"
        );
    }

    #[test]
    fn folded_marker_is_treated_as_literal() {
        // SYON has no folded style; `>` means `|`, so newlines survive.
        assert_eq!(scalar_of("a: >\n  one\n  two\n", "a"), "one\ntwo\n");
    }

    #[test]
    fn block_scalar_chomping() {
        assert_eq!(scalar_of("a: |-\n  one\n", "a"), "one");
        assert_eq!(scalar_of("a: |\n  one\n", "a"), "one\n");
    }

    #[test]
    fn continuation_lines_fold_into_the_scalar() {
        assert_eq!(
            scalar_of("a: cargo run --manifest-path x/Cargo.toml\n            -p app -- \"y\"\n", "a"),
            "cargo run --manifest-path x/Cargo.toml -p app -- \"y\""
        );
    }

    #[test]
    fn a_child_block_is_not_a_continuation() {
        // Structural lines nest; only non-structural deeper lines fold.
        let doc = parse_document("outer:\n  inner: 1\n").unwrap();
        let Value::Mapping(entries) = doc.body else { panic!() };
        assert!(matches!(entries[0].value, Value::Mapping(_)));
    }

    #[test]
    fn comment_only_document_terminates() {
        // Regression: this looped forever, because the body parser gathers
        // leading comments then rewinds when no entry follows.
        assert!(parse("# just a comment\n").is_ok());
        assert!(parse("# one\n# two\n\n").is_ok());
    }

    #[test]
    fn a_leading_document_marker_is_allowed_but_a_second_is_not() {
        assert!(parse("---\nkey: value\n").is_ok());
        assert!(parse("key: value\n---\nother: value\n").is_err());
    }

    #[test]
    fn structural_indent_must_match_space_count() {
        // Default step is 2.
        assert!(parse("a:\n  b: 1\n").is_ok());
        let err = parse("a:\n   b: 1\n").unwrap_err().to_string();
        assert!(err.contains("multiple of 2"), "got: {err}");

        // A block body is content, so it is exempt from the step.
        assert!(parse_with("a: |\n   ragged\n     body\n", ParseOptions::default()).is_ok());
    }

    #[test]
    fn multi_space_before_a_trailing_comment() {
        // Writers align comments into a column.
        assert_eq!(scalar_of("a: \"x\"   # note\n", "a"), "x");
    }

    #[test]
    fn compact_mapping_in_a_sequence_needs_the_option() {
        let opts = ParseOptions { allow_key_in_line_after_list: true, ..Default::default() };

        // Off (default): the line is prose, and stays text.
        let doc = parse_document("deps:\n  - task: build\n").unwrap();
        let Value::Mapping(e) = doc.body else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        assert_eq!(items[0].value, Value::Scalar("task: build".into()));

        // On: the same line is a one-entry mapping.
        let f = parse_with("deps:\n  - task: build\n", opts).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        let Value::Mapping(entry) = &items[0].value else { panic!("expected a mapping") };
        assert_eq!(entry[0].key, "task");
        assert_eq!(entry[0].value, Value::Scalar("build".into()));
    }

    #[test]
    fn compact_mapping_merges_with_its_deeper_entries() {
        // Regression: the inline half used to be dropped outright, so
        // `task:` vanished from a parameterised dependency with no error.
        let opts = ParseOptions { allow_key_in_line_after_list: true, ..Default::default() };
        let f = parse_with("deps:\n  - task: build\n    vars:\n      A: 1\n", opts).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        let Value::Mapping(entry) = &items[0].value else { panic!("expected a mapping") };
        let keys: Vec<&str> = entry.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["task", "vars"], "the inline entry must survive");
    }

    #[test]
    fn inline_text_plus_a_block_is_an_error_while_the_option_is_off() {
        let err = parse("deps:\n  - task: build\n    vars:\n      A: 1\n")
            .unwrap_err()
            .to_string();
        assert!(err.contains("allow_key_in_line_after_list"), "got: {err}");
    }

    // --- `- key: |` block scalars in compact entries ---

    fn compact(src: &str) -> Vec<MappingEntry> {
        let opts = ParseOptions { allow_key_in_line_after_list: true, ..Default::default() };
        let f = parse_with(src, opts).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!("expected a sequence") };
        let Value::Mapping(entry) = &items[0].value else { panic!("expected a mapping") };
        entry.clone()
    }

    /// The same document written with the key on its own line, which has
    /// always been correct. A compact entry must agree with it.
    fn expanded(src: &str) -> Vec<MappingEntry> {
        let f = parse(src).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!("expected a sequence") };
        let Value::Mapping(entry) = &items[0].value else { panic!("expected a mapping") };
        entry.clone()
    }

    #[test]
    fn a_compact_entry_takes_a_one_line_block_scalar() {
        let e = compact("cmds:\n  - md: |\n      hello world\n");
        assert_eq!(e[0].key, "md");
        assert_eq!(e[0].value, Value::LiteralBlock("hello world\n".into()));
        assert_eq!(e[0].value, expanded("cmds:\n  -\n    md: |\n      hello world\n")[0].value);
    }

    #[test]
    fn a_compact_entry_takes_a_multi_line_block_scalar() {
        // Previously folded to the single scalar "| hello world".
        let e = compact("cmds:\n  - md: |\n      hello\n      world\n");
        assert_eq!(e[0].value, Value::LiteralBlock("hello\nworld\n".into()));
        assert_eq!(
            e[0].value,
            expanded("cmds:\n  -\n    md: |\n      hello\n      world\n")[0].value
        );
    }

    #[test]
    fn a_compact_block_scalar_body_may_begin_with_a_hash() {
        // The reproduction a naive fix still gets wrong: `# Heading` lexes as
        // a comment and is discarded, leaving the bare header "|" as the
        // value. take_block_scalar reads source lines, so it is unaffected.
        let e = compact("cmds:\n  - md: |\n      # Heading\n      text\n");
        assert_eq!(e[0].value, Value::LiteralBlock("# Heading\ntext\n".into()));
        assert_eq!(
            e[0].value,
            expanded("cmds:\n  -\n    md: |\n      # Heading\n      text\n")[0].value
        );
    }

    #[test]
    fn a_compact_block_scalar_chomps_like_any_other() {
        let strip = compact("cmds:\n  - md: |-\n      hello\n");
        assert_eq!(strip[0].value, Value::LiteralBlock("hello".into()));

        let keep = compact("cmds:\n  - md: |+\n      hello\n\n");
        assert_eq!(keep[0].value, Value::LiteralBlock("hello\n".into()));

        // `>` is a spelling of `|`, never a folded scalar.
        let folded = compact("cmds:\n  - md: >\n      hello\n      world\n");
        assert_eq!(folded[0].value, Value::LiteralBlock("hello\nworld\n".into()));
    }

    #[test]
    fn a_sibling_entry_ends_a_compact_block_scalar() {
        // `sh:` aligns under `md`, so it is the next entry of the same compact
        // mapping -- not another line of the block.
        let e = compact("cmds:\n  - md: |\n      hello\n    sh: echo hi\n");
        let keys: Vec<&str> = e.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["md", "sh"]);
        assert_eq!(e[0].value, Value::LiteralBlock("hello\n".into()));
        assert_eq!(e[1].value, Value::Scalar("echo hi".into()));
    }

    // --- `- key:` whose value is a nested block ---

    /// Entries with every source line cleared. The expanded form puts the
    /// `-` on a line of its own, so its lines are one later; only the shape
    /// has to agree.
    fn shape(entries: Vec<MappingEntry>) -> Vec<MappingEntry> {
        fn value(v: Value) -> Value {
            match v {
                Value::Mapping(m) => Value::Mapping(shape(m)),
                Value::Sequence(items) => Value::Sequence(
                    items
                        .into_iter()
                        .map(|i| SequenceItem { line: 0, value: value(i.value), ..i })
                        .collect(),
                ),
                other => other,
            }
        }
        entries
            .into_iter()
            .map(|e| MappingEntry { line: 0, value: value(e.value), ..e })
            .collect()
    }

    #[test]
    fn a_compact_entry_takes_a_nested_sequence() {
        let src = "groups:\n  - tabs:\n      - viewer\n      - help\n";
        let e = compact(src);
        assert_eq!(
            shape(e.clone()),
            shape(expanded("groups:\n  -\n    tabs:\n      - viewer\n      - help\n"))
        );
        let Value::Sequence(tabs) = &e[0].value else { panic!("tabs should be a sequence: {e:?}") };
        assert_eq!(tabs.len(), 2);
    }

    #[test]
    fn a_nested_sequence_keeps_the_compact_entries_after_it() {
        // `side:` aligns under `tabs`, so it is the next entry of the same
        // compact mapping; the deeper `- params` lines are the value of `tabs`.
        let src = "groups:\n  - tabs:\n      - params\n      - code\n    side: right\n    size: 35%\n";
        let e = compact(src);
        let keys: Vec<&str> = e.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["tabs", "side", "size"]);
        assert_eq!(
            shape(e),
            shape(expanded(
                "groups:\n  -\n    tabs:\n      - params\n      - code\n    side: right\n    size: 35%\n"
            ))
        );
    }

    #[test]
    fn a_compact_entry_takes_a_nested_mapping() {
        let src = "cmds:\n  - env:\n      A: 1\n      B: 2\n    sh: echo hi\n";
        assert_eq!(
            shape(compact(src)),
            shape(expanded("cmds:\n  -\n    env:\n      A: 1\n      B: 2\n    sh: echo hi\n"))
        );
    }

    #[test]
    fn a_nested_sequence_may_follow_other_compact_entries() {
        let src = "groups:\n  - side: right\n    tabs:\n      - params\n";
        assert_eq!(
            shape(compact(src)),
            shape(expanded("groups:\n  -\n    side: right\n    tabs:\n      - params\n"))
        );
    }

    #[test]
    fn an_empty_compact_entry_followed_by_siblings_stays_empty() {
        // Nothing deeper than the key column: `task:` has no value, and the
        // aligned `vars:` is its sibling, as before.
        let e = compact("deps:\n  - task:\n    vars:\n      A: 1\n");
        let keys: Vec<&str> = e.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(keys, ["task", "vars"]);
        assert_eq!(e[0].value, Value::Scalar(String::new()));
    }

    #[test]
    fn cmd_is_the_same_case_as_md() {
        // The bug predates `md:`; it was always about `- key: |`.
        let e = compact("cmds:\n  - cmd: |\n      echo one\n      echo two\n");
        assert_eq!(e[0].key, "cmd");
        assert_eq!(e[0].value, Value::LiteralBlock("echo one\necho two\n".into()));
    }

    #[test]
    fn a_compact_block_scalar_errors_while_the_option_is_off() {
        // Without the option the body would be folded into the scalar and the
        // `|` left as text. An error naming the option beats a wrong value.
        let err = parse("cmds:\n  - md: |\n      hello\n").unwrap_err().to_string();
        assert!(err.contains("allow_key_in_line_after_list"), "got: {err}");
    }

    #[test]
    fn only_the_first_colon_space_splits_a_compact_entry() {
        let opts = ParseOptions { allow_key_in_line_after_list: true, ..Default::default() };
        let f = parse_with("cmds:\n  - task: core:info\n", opts).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        let Value::Mapping(entry) = &items[0].value else { panic!() };
        assert_eq!(entry[0].value, Value::Scalar("core:info".into()));
    }

    #[test]
    fn a_quoted_colon_does_not_split_a_compact_entry() {
        let opts = ParseOptions { allow_key_in_line_after_list: true, ..Default::default() };
        let f = parse_with("cmds:\n  - echo \"a: b\"\n", opts).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        assert_eq!(items[0].value, Value::Scalar("echo \"a: b\"".into()));
    }

    #[test]
    fn single_quoted_scalars_are_unwrapped() {
        // `'...'` is literal throughout, and `''` is one apostrophe.
        let doc = parse_document("a: '3'\nb: 'it''s here'\nc: '{{ .X }}'\n").unwrap();
        let Value::Mapping(e) = doc.body else { panic!() };
        assert_eq!(e[0].value, Value::Scalar("3".into()));
        assert_eq!(e[1].value, Value::Scalar("it's here".into()));
        assert_eq!(e[2].value, Value::Scalar("{{ .X }}".into()));
    }

    #[test]
    fn spaces_after_a_block_1_symbol_are_trimmed() {
        // `: ` and `- ` consume exactly one space, so any alignment padding
        // after them would otherwise land inside the value.
        let doc = parse_document("a:   ./module1\nb:     x\n").unwrap();
        let Value::Mapping(e) = doc.body else { panic!() };
        assert_eq!(e[0].value, Value::Scalar("./module1".into()));
        assert_eq!(e[1].value, Value::Scalar("x".into()));

        let doc = parse_document("items:\n  -   spaced\n").unwrap();
        let Value::Mapping(e) = doc.body else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        assert_eq!(items[0].value, Value::Scalar("spaced".into()));
    }

    #[test]
    fn quoting_preserves_spaces_a_plain_scalar_would_lose() {
        let doc = parse_document("a: \"  kept  \"\n").unwrap();
        let Value::Mapping(e) = doc.body else { panic!() };
        assert_eq!(e[0].value, Value::Scalar("  kept  ".into()));
    }

    #[test]
    fn a_compact_entry_unquotes_its_value() {
        // `- sh: '[ x = y ]'` must reach the consumer as a runnable command,
        // not as a single quoted word.
        let opts = ParseOptions { allow_key_in_line_after_list: true, ..Default::default() };
        let f = parse_with("pre:\n  - sh: '[ \"a\" = a ]'\n", opts).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        let Value::Mapping(entry) = &items[0].value else { panic!("expected a mapping") };
        assert_eq!(entry[0].value, Value::Scalar("[ \"a\" = a ]".into()));
    }

    #[test]
    fn a_compact_entry_keeps_an_unquoted_value_intact() {
        let opts = ParseOptions { allow_key_in_line_after_list: true, ..Default::default() };
        let f = parse_with("deps:\n  - task: build\n", opts).unwrap();
        let Value::Mapping(e) = f.documents[0].body.clone() else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        let Value::Mapping(entry) = &items[0].value else { panic!() };
        assert_eq!(entry[0].value, Value::Scalar("build".into()));
    }

    #[test]
    fn entries_carry_their_source_line() {
        // Without a line, "unrecognised field `x`" points at a whole file.
        let doc = parse_document("a: 1\nb: 2\nc:\n  d: 3\n").unwrap();
        let Value::Mapping(e) = doc.body else { panic!() };
        assert_eq!(e[0].line, 1);
        assert_eq!(e[1].line, 2);
        assert_eq!(e[2].line, 3);
        let Value::Mapping(inner) = &e[2].value else { panic!() };
        assert_eq!(inner[0].line, 4);
    }

    #[test]
    fn sequence_items_carry_their_source_line() {
        let doc = parse_document("items:\n  - a\n  - b\n").unwrap();
        let Value::Mapping(e) = doc.body else { panic!() };
        let Value::Sequence(items) = &e[0].value else { panic!() };
        assert_eq!(items[0].line, 2);
        assert_eq!(items[1].line, 3);
    }

    // --- Nested structure stress tests ---

    #[test]
    fn sequence_of_mappings() {
        let input = "people:\n  -\n    name: alice\n    age: 30\n  -\n    name: bob\n    age: 25\n";
        let doc = parse_document(input).unwrap();
        let Value::Mapping(root) = &doc.body else { panic!("expected root Mapping") };
        assert_eq!(root[0].key, "people");
        let Value::Sequence(items) = &root[0].value else { panic!("expected Sequence") };
        assert_eq!(items.len(), 2);
        let Value::Mapping(first) = &items[0].value else { panic!("expected item Mapping") };
        assert_eq!(first[0].key, "name");
        assert_eq!(first[0].value, Value::Scalar("alice".into()));
        assert_eq!(first[1].key, "age");
        assert_eq!(first[1].value, Value::Scalar("30".into()));
        let Value::Mapping(second) = &items[1].value else { panic!("expected item Mapping") };
        assert_eq!(second[0].key, "name");
        assert_eq!(second[0].value, Value::Scalar("bob".into()));
    }

    #[test]
    fn triple_nested_map_seq_map() {
        let input = "config:\n  items:\n    -\n      key: value\n      extra: data\n";
        let doc = parse_document(input).unwrap();
        let Value::Mapping(root) = &doc.body else { panic!("expected root Mapping") };
        let Value::Mapping(config) = &root[0].value else { panic!("expected config Mapping") };
        assert_eq!(config[0].key, "items");
        let Value::Sequence(items) = &config[0].value else { panic!("expected Sequence") };
        assert_eq!(items.len(), 1);
        let Value::Mapping(inner) = &items[0].value else { panic!("expected inner Mapping") };
        assert_eq!(inner[0].key, "key");
        assert_eq!(inner[0].value, Value::Scalar("value".into()));
        assert_eq!(inner[1].key, "extra");
        assert_eq!(inner[1].value, Value::Scalar("data".into()));
    }

    #[test]
    fn sibling_sequences_at_different_depths() {
        let input = "top_list:\n  - a\n  - b\nnested:\n  inner_list:\n    - c\n    - d\n";
        let doc = parse_document(input).unwrap();
        let Value::Mapping(root) = &doc.body else { panic!("expected root Mapping") };
        assert_eq!(root.len(), 2);
        let Value::Sequence(top) = &root[0].value else { panic!("expected top Sequence") };
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].value, Value::Scalar("a".into()));
        let Value::Mapping(nested) = &root[1].value else { panic!("expected nested Mapping") };
        let Value::Sequence(inner) = &nested[0].value else { panic!("expected inner Sequence") };
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0].value, Value::Scalar("c".into()));
    }

    #[test]
    fn mixed_block_scalars_and_sequence() {
        let input = "root:\n  label: hello\n  items:\n    - one\n    - two\n  count: 3\n";
        let doc = parse_document(input).unwrap();
        let Value::Mapping(root) = &doc.body else { panic!("expected root Mapping") };
        let Value::Mapping(inner) = &root[0].value else { panic!("expected inner Mapping") };
        assert_eq!(inner.len(), 3);
        assert_eq!(inner[0].key, "label");
        assert_eq!(inner[0].value, Value::Scalar("hello".into()));
        assert_eq!(inner[1].key, "items");
        let Value::Sequence(items) = &inner[1].value else { panic!("expected Sequence") };
        assert_eq!(items.len(), 2);
        assert_eq!(inner[2].key, "count");
        assert_eq!(inner[2].value, Value::Scalar("3".into()));
    }

    #[test]
    fn dedent_to_root_after_deep_nesting() {
        let input = "deep:\n  level1:\n    level2:\n      leaf: value\nback_at_root: yes\n";
        let doc = parse_document(input).unwrap();
        let Value::Mapping(root) = &doc.body else { panic!("expected root Mapping") };
        assert_eq!(root.len(), 2);
        assert_eq!(root[0].key, "deep");
        assert_eq!(root[1].key, "back_at_root");
        assert_eq!(root[1].value, Value::Scalar("yes".into()));
        let Value::Mapping(l1) = &root[0].value else { panic!("expected level1 Mapping") };
        let Value::Mapping(l2) = &l1[0].value else { panic!("expected level2 Mapping") };
        let Value::Mapping(leaf_map) = &l2[0].value else { panic!("expected leaf Mapping") };
        assert_eq!(leaf_map[0].key, "leaf");
        assert_eq!(leaf_map[0].value, Value::Scalar("value".into()));
    }
}
