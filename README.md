# ONE Mono Repository

## Workspace layout

```
crates/
  syon-parser/     # pest-based parser and emitter, AST in both directions
  syon-cli/        # `syon` binary — parses a .syon file and prints the AST as JSON
  syon-python/     # PyO3 bindings exposing syon-parser to Python
  sheni_types/     # Sheni (layer 2), the type layer over the parsed AST
  shelishi_schema/ # Shelishi (layer 3), user-declared types from a runtime schema
  luach_types/     # calendar-agnostic day count, week, and the calendar contract
  hodesh_calendar/ # the calendars themselves — Gregorian, and hodesh
  shlita_types/    # IEC 61131-3 elementary types and standard functions
syon-go/           # independent, dependency-free Go implementation
spec/              # language specification
```

**A crate's directory, its package, and its library all carry the same name.**
`cargo build -p sheni_types`, `sheni_types = { path = "../sheni_types" }` and
`use sheni_types::…` are then the same word, so there is nothing to look up.
Two crates used to have three names between them — package `sheni` in directory
`sheni_types` with library `sheni_types` — and every one of the three was wrong
somewhere.

The `syon-*` crates keep Cargo's own convention, where a hyphenated package has
an underscored library: `syon-parser` builds `syon_parser`. `syon-python` is the
one deliberate exception, whose library is `syon` because PyO3 requires the
cdylib to be named for the module Python imports.

## Quick start

```bash
task build-parser
task run-cli-binary -- examples/glossary/entries/syon.syon
```

## Examples

| Example | Shows |
|---------|-------|
| [`examples/glossary/`](examples/glossary) | A glossary entry and its schema — the language itself |
| [`examples/planner/`](examples/planner) | A task tracker using the type layer, including the soft types. Read through Sheni by `crates/sheni_types/tests/planner_example.rs`, so it cannot drift from the crate. |

## Spec

See [`spec/README.md`](spec/README.md) for the full language specification.

## Architecture decisions

See [`design/architecture/`](design/architecture) for the ADR log — why pest was
chosen for the Rust parser, why Go got an independent implementation
instead of FFI bindings, and other architecturally significant calls.

Each layer numbers its own decisions and the prefix says which layer:
`syon` for the parser and the language, `sheni` for the type layer above it,
`shelishi` for the schema layer above that, `hodesh` for the calendar
crates and `shlita` for the IEC 61131-3 control crates, both of which sit
alongside rather than on top of the SYON stack.

| ADR | Title |
|-----|-------|
| [syon_01](design/architecture/ADR_syon_01__record_architecture_decission.syon) | Record architecture decisions |
| [syon_02](design/architecture/ADR_syon_02__pest_as_the_rust_parsing_engine.syon) | Use pest as the Rust parsing engine |
| [syon_03](design/architecture/ADR_syon_03__preflight_scan_for_forbidden_constructs.syon) | Preflight text scan for forbidden constructs |
| [syon_04](design/architecture/ADR_syon_04__independent_go_implementation.syon) | Independent Go implementation instead of FFI bindings |
| [syon_05](design/architecture/ADR_syon_05__block_1_only_yaml_compatibility.syon) | Only Block 1 is YAML-compatible |
| [syon_06](design/architecture/ADR_syon_06__phase1_block_numbering.syon) | Phase1 report uses its own block numbering, distinct from the grammar spec |
| [syon_07](design/architecture/ADR_syon_07__remove_the_literal_escape_hatch.syon) | Remove the literal escape hatch in favour of YAML block scalars |
| [syon_08](design/architecture/ADR_syon_08__parse_error_codes.syon) | Numeric error codes for parse errors, mirrored per language |
| [syon_09](design/architecture/ADR_syon_09__split_and_compact_multi_document.syon) | Split and compact subcommands for multi-document files |
| [hodesh_01](design/architecture/ADR_hodesh_01__two_crates_and_a_day_count.syon) | Calendars convert through one day count, and the count lives in a crate below them |
| [hodesh_02](design/architecture/ADR_hodesh_02__month_is_the_mean_lunation.syon) | The month is the mean lunation, and the 29/30 alternation is its consequence rather than its rule |
| [hodesh_03](design/architecture/ADR_hodesh_03__metonic_year_and_appended_leap_month.syon) | The year is Metonic, and the leap month is appended as month 13 rather than inserted |
| [hodesh_04](design/architecture/ADR_hodesh_04__numbered_months_and_a_new_moon_epoch.syon) | Months are numbered rather than named, and year zero begins at the first new moon of 2000 |
| [shelishi_01](design/architecture/ADR_shelishi_01__schema_based_types.syon) | The schema layer supplies types at runtime, reusing the shapes sheni already defines |
| [sheni_01](design/architecture/ADR_sheni_01__primitives.syon) | Primitive type set, spelling, and literal forms for Sheni |
| [sheni_02](design/architecture/ADR_sheni_02__simple_types.syon) | Simple types delegate to the standard crates, and reading them normalises |
| [sheni_03](design/architecture/ADR_sheni_03__complex_types.syon) | Complex types are a curated library shipped in code, not a way to declare types |
| [sheni_04](design/architecture/ADR_sheni_04__collections.syon) | Collections are parameterised by their element types, and a map key is read like any other value |
| [sheni_05](design/architecture/ADR_sheni_05__type_layer_boundary.syon) | The type layer stops at four kinds, and user-defined types belong to the layer above |
| [sheni_06](design/architecture/ADR_sheni_06__soft_dates.syon) | A date that is not fully known is a type, not a date field that might be missing |
| [sheni_07](design/architecture/ADR_sheni_07__soft_bool.syon) | A boolean that may be unknown is a three-valued enum, and `bool` stays two-valued |
| [sheni_08](design/architecture/ADR_sheni_08__soft_types.syon) | An unknown belongs in the value space, and the `soft_` prefix marks the types that hold one |
| [sheni_09](design/architecture/ADR_sheni_09__soft_primitives.syon) | Every primitive gets a soft twin except `string`, which cannot spell its own unknown |
| [sheni_10](design/architecture/ADR_sheni_10__soft_date_ranges.syon) | A range whose ends may be unknown is one type, and the standard chooses its fallback |
| [shlita_01](design/architecture/ADR_shlita_01__two_crates_and_scope.syon) | IEC 61131-3 splits into a type vocabulary and a scan runtime, and neither one is a language |
| [shlita_02](design/architecture/ADR_shlita_02__fbd_and_sfc_are_documents.syon) | FBD and SFC are documents rather than syntaxes, and PLCopen already names their parts |
| [shlita_03](design/architecture/ADR_shlita_03__structured_text_dialect.syon) | Structured Text takes Python's statements and IEC's types, and is not Python |
| [shlita_04](design/architecture/ADR_shlita_04__engine_owns_its_drivers.syon) | The engine owns the image and its drivers, and one scan is three calls rather than one |
| [shlita_05](design/architecture/ADR_shlita_05__substitute_values_and_quality.syon) | A signal that cannot be read keeps a defined value and gains a bad quality, and the reaction to that is declared per module |

### Printable edition

`task adr-pdf` renders every record into `build/ADR__<date>.pdf` — a typeset
book with a cover, a linked index and PDF bookmarks. The pipeline reads the
records through `syon-cli` rather than a second parser, so the book is built
from the same bytes CI validates, and it takes its order from the index table
above and fails if a record on disk is missing from it. Requires
[Typst](https://typst.app); the template is
[`design/architecture/typst/adr_book.typ`](design/architecture/typst/adr_book.typ).

## Roadmap / TODO

- [ ] Add a YAML-compatible mode for Block 2 (document fences), so fenced
      sub-documents can be parsed by a plain YAML 1.2 parser instead of
      requiring SYON-specific fence syntax.
      ([#2](https://github.com/object-notation-environment/safe-yaml-object-notation/issues/2))
- [x] Add a YAML-compatible mode for Block 3 (literal escape hatch), treating
      it as equivalent to a YAML multiline block scalar (`|`) rather than the
      SYON-specific `[[[`/`]]]` delimiters. Done, and further: `[[[` was
      removed outright rather than kept alongside `|`, so there is no mode to
      choose. See ADR 0007.
      ([#3](https://github.com/object-notation-environment/safe-yaml-object-notation/issues/3))

## Documentation

The [`docs/`](docs) directory holds a [Zensical](https://zensical.org)
documentation site (getting started, language guide, CLI reference, Python
bindings, and the glossary example). Build it with:

```bash
pip install zensical
task docs-serve   # local live-reload preview at http://localhost:8000
task docs-build   # static site in site/
```

On push to `main`, [`.github/workflows/docs.yml`](.github/workflows/docs.yml)
builds and publishes the site to GitHub Pages at
<https://object-notation-environment.github.io/safe-yaml-object-notation/>.
This requires enabling Pages once, in the repo's **Settings → Pages**, with
**Source** set to **GitHub Actions**.

## License

MIT


## SYON — Safe YAML Object Notation

SYON is a YAML-inspired, minimal object-notation language designed for safety and predictability.
Its core record syntax supports the data model of YAML — scalars, sequences, and mappings —
while deliberately excluding anchors, aliases, and arbitrary tags. That core syntax alone is a
strict, safe subset of YAML; SYON as a whole is not, since it adds document fences and literal
blocks with no YAML equivalent. See [`spec/README.md`](spec/README.md#relationship-to-yaml) for
the full picture.

### Goals

- **Safe**: no executable directives, no reference cycles, no arbitrary type coercion.
- **Readable**: indentation-based, human-friendly syntax.
- **Embeddable**: a single Rust library crate with no unsafe code.
