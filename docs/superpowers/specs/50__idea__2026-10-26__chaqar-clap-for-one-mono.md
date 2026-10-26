# Check whether chaqar-clap helps one__mono

Date: 2026-10-26
Status: idea

Depends on:
  - nothing.
Needed by:
  - nothing.

## TODO

Check whether chaqar-clap would be useful in one__mono, and if so which of its
crates should use it.

chaqar-clap is the drop-in replacement for clap in `beriah__library__mono`
(`crates/chaqar-clap`, branch `feat/chaqar-clap`). A tool switches by changing its
imports, and the suite's later layers (questions, information texts, shared
completion) then reach it without a second migration. See `ADR-Chaqar-Clap-01`.

## What to look at

- `crates/syon-cli` (binary `syon`) reads `std::env::args()` by hand and has no
  command line library at all. It has no `--help` or `--version` of its own that a
  parser would give it. This is the most likely candidate.
- `crates/tsv-syon` and any other binary crate: does it take arguments?
- The Python and Go sides (`syon-python`, `syon-go`) are out of scope: chaqar-clap
  is pure Rust, and the Python API stays in Chaqar.

## Questions to answer

- Does one__mono want a dependency on the library repo at all? It sits *below*
  the library in the suite (the library and the maintenance tools depend on
  `one__mono`), so a path dependency back up could make a cycle.
- If it would, is a hand-written `env::args()` loop cheaper than the dependency
  for a CLI this small?
- Would `syon` benefit from the later layers (shared completion, tutorial texts),
  or is it a plain tool that stays plain?

This is an idea, not a spec: nothing is decided and nothing is to be built until the
questions above have an answer.
