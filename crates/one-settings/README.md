# one-settings

The user's shared settings file for the Beriah suite: one `settings.syon` in the
user's home directory, read by every application in the suite.

The point of the crate is that the file is **one** file. Before it existed each
frontend carried its own hardcoded interface constants, so making the text
larger meant editing — and rebuilding — every application separately. Now the
number lives on disk, outside the binaries, and one edit moves the whole suite.

## Where the file lives

Candidate paths, in order; the first that exists wins:

1. `$BERIAH_SETTINGS_FILE` — an explicit full path to a settings file.
2. `$XDG_CONFIG_HOME/beriah/settings.syon`
3. `$HOME/.config/beriah/settings.syon`
4. `$HOME/.beriah/settings.syon`

If none of them exists, the built-in defaults apply. **A missing file is not an
error** — running the suite without ever writing a settings file is the normal
case, and every key is optional.

A malformed file *is* an error. A typo that silently reverted your settings to
the defaults would be worse than a message saying which key is wrong, so
[`load`] returns the parse or validation failure and lets the caller decide
whether to fall back.

## The file

```yaml
version: 1
interface:
  font_size: 18
features:
  departures: true
storage:
  reports:
    path: ~/beriah/reports
    remote: git@github.com:7solultions/beriah-reports.git
  teams:
    path: ~/beriah/teams
    remote: git@github.com:7solultions/beriah-teams.git
  pakad:
    path: ~/beriah/pakad
    remote: git@github.com:7solultions/beriah-pakad.git
```

| Key                      | Type    | Default  | Meaning                                         |
|--------------------------|---------|----------|-------------------------------------------------|
| `interface.font_size`    | integer | `18`     | Base body-text size in points (range 6 – 96).  |
| `features.departures`    | boolean | `false`  | Offer the process for team members leaving.     |
| `storage.reports.path`   | path    | *unset*  | Where `beriah-support` writes reports.          |
| `storage.reports.remote` | URL     | *unset*  | The git remote that folder shares through.      |
| `storage.teams.path`     | path    | *unset*  | Where `beriah-team` keeps rosters.              |
| `storage.teams.remote`   | URL     | *unset*  | The git remote that folder shares through.      |
| `storage.pakad.path`     | path    | *unset*  | Where Pakad keeps its configuration and history.|
| `storage.pakad.remote`   | URL     | *unset*  | Declared by settings; Pakad does not apply yet.|

### Where the data lives

Every key under `storage:` is optional, and an absent one means each application
keeps whatever fallback it had before the section existed. A leading `~/` is
expanded against `$HOME`, because a settings file that has to spell out
`/Users/somebody` is one that cannot be copied to another machine.

**Reports and rosters are named separately and never share a default.** Rosters
are the one place in the suite that maps a participant slot to a real person;
reports travel to an outside support team. Pointing both at one folder would
undo, in a single line of configuration, the separation the crates are
structured to guarantee.

Pakad has its own folder setting and keeps it separate as well, though not for
privacy reasons — it holds configuration and review history, not personal data.

A command-line argument or the application's own environment variable
(`BERIAH_SUPPORT_REPORTS_ROOT`, `BERIAH_TEAM_ROSTER_ROOT`) beats the settings
file, for the same reason `BERIAH_SETTINGS_FILE` exists: a one-off run has to be
able to point somewhere else without editing, then restoring, real
configuration.

### A declared remote is applied, not merely checked

Naming a `remote:` makes the application run `git remote add` — or `set-url`, if
it already points elsewhere — on the folder's repository at start-up. That is
how a deployment configures sharing centrally instead of talking every user
through git on their own laptop, and it is worth being clear about what it
means: **whoever writes this file decides where that data is pushed.**

Two things it will not do:

- **It will not touch a repository the folder merely sits inside.** A remote
  belongs to a whole repository, not to a folder, so a reports folder living in
  a source checkout would otherwise repoint that checkout's `origin`. The
  application refuses and switches sharing off instead.
- **It will not create a repository.** No `clone`, no `init`. A folder is either
  already a repository or sharing is off for it.

If a declared remote cannot be applied, sharing is switched off and the window
says why. Whatever `origin` happened to point at is not a remote anybody chose
for that data, and pushing to it is the failure the refusal exists to prevent.

### Features are off until asked for

Every flag under `features:` defaults to **false**. A switch exists so a
capability can ship without being turned on; a default of `true` would invert
that and make the file opt *out* of something nobody asked for.

An unknown flag is ignored, so an older build will not refuse a file that
mentions a switch it has never heard of. A flag that is present but is not a
boolean is an error rather than a silent `false` — somebody who writes
`departures: yes` meant to switch something on, and quietly leaving it off is
the one outcome that teaches them nothing.

`features.departures` gates the *editing* of departures, not the reading of
them. With it off, `beriah-team` still loads a roster in full and still shows
who has departed and where their work went; what disappears is every control
that would change it.

`font_size` is the size of *body* text. Applications scale their whole text
hierarchy — heading, button, small, monospace — by the same factor, so headings
stay proportionally larger rather than collapsing to one flat size. See
`apply_font_size` in the egui frontends for how that factor is derived.

The size is an integer because `read_integer` (in `srcs/scalar.rs`) reads it
through Sheni's `i64`, the suite's one spelling of "this raw string is a
number", and there is no float reader to reuse. Point sizes are whole numbers
in practice. `-0` is rejected, and so is any boolean that is not exactly `true`
or `false`: `syon-parser` only parses, and the meaning of a scalar is decided
here, in the application layer.

## The other file: `info.syon`

The crate also reads `info.syon` from the same directory — see the [`info`
module](srcs/info.rs). It holds *who the person is*:

```yaml
# ~/.config/beriah/info.syon
company-id: c001
team-id: p1
```

They are two files rather than one on purpose. Preferences are portable —
copyable between machines, reasonable to keep in a dotfiles repository —
and an identity is not. Both keys are optional, and the values are handed back
as **raw strings**: this crate does not know what a company id or a participant
slot is, and deliberately cannot resolve either to a person.

## Usage

```rust
let settings = one_settings::load().unwrap_or_default();
let size = settings.interface.font_size;
```

Missing keys fall back to defaults individually, so a file that sets only
`interface.font_size` leaves every future key at its default.
