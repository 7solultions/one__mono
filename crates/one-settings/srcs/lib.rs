//! The user's shared settings file for the Beriah suite.
//!
//! Every application in the suite reads one `settings.syon` from the user's
//! home directory. That is the whole point of this crate: the interface
//! preferences that used to be hardcoded constants inside each frontend now
//! live in a single file on disk, so one edit moves the whole suite and no
//! rebuild is involved.
//!
//! This crate is layer two over [`syon_parser`] in exactly the sense the
//! repository means it: layer one hands up raw strings, and every "this string
//! is a number, and it is in range" decision is made here, once, rather than in
//! each application.
//!
//! ## What a missing file means
//!
//! Nothing. A user who never writes a settings file is the normal case, so
//! every key is optional and an absent file yields [`Settings::default`].
//!
//! A file that *exists* but cannot be read is a different matter and is
//! reported as an error. Silently ignoring a typo would hand the user back the
//! defaults with no explanation of why their setting did not take, which is the
//! worst of the three outcomes. [`load`] therefore surfaces the failure and
//! lets the caller decide whether to fall back — frontends generally do fall
//! back, after printing the message.
//!
//! ## Resolution order
//!
//! See [`settings_path`]. `BERIAH_SETTINGS_FILE` wins so that tests and
//! one-off runs can point at a file without touching the user's real one.
//!
//! ## The two files
//!
//! The suite keeps two files in that directory, and the split is on purpose:
//!
//! * `settings.syon` — *preferences*, this module. Losing it costs nothing but
//!   a font size and a switched-off feature.
//! * `info.syon` — *who the person is*, [`info`]. It holds a company id and a
//!   participant slot, never a name.
//!
//! Preferences are shared and portable; identity is not. Keeping them apart
//! means a settings file can be copied between machines, or checked into a
//! dotfiles repository, without carrying an identity along with it.

pub mod info;
pub mod scalar;

use std::path::{Path, PathBuf};

use syon_parser::Mapping;
use thiserror::Error;

use crate::scalar::{read_boolean, read_integer, ScalarError};

/// The environment variable holding an explicit path to a settings file. It
/// names the *file*, not its directory, so a caller can point at anything.
pub const SETTINGS_FILE_ENVIRONMENT_VARIABLE: &str = "BERIAH_SETTINGS_FILE";

/// The file name looked for inside each candidate configuration directory.
pub const SETTINGS_FILE_NAME: &str = "settings.syon";

/// The directory the suite owns inside the user's configuration root.
pub const SETTINGS_DIRECTORY_NAME: &str = "beriah";

/// The default base body-text size, in points.
///
/// Deliberately larger than the toolkit defaults the frontends would otherwise
/// inherit: the suite is a long-reading desktop application, and the setting
/// exists precisely because those defaults were too small.
pub const DEFAULT_FONT_SIZE: i64 = 18;

/// The accepted range for [`InterfaceSettings::font_size`], in points.
///
/// The bounds are not taste, they are damage control: a stray `0` or `1000` in
/// the file would render the suite unusable, and a window whose text cannot be
/// read is a window in which the settings file cannot be corrected.
pub const FONT_SIZE_RANGE: std::ops::RangeInclusive<i64> = 6..=96;

/// Everything the shared settings file can declare.
///
/// Its `Default` is every section's default, which is what an absent settings
/// file resolves to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// Interface preferences shared by every frontend in the suite.
    pub interface: InterfaceSettings,
    /// Which optional capabilities are switched on.
    pub features: FeatureSettings,
    /// Where each application's data folder lives and which remote it shares
    /// through.
    pub storage: StorageSettings,
    /// Colour preferences for command-line output, as plain words.
    pub colors: ColorSettings,
}

/// The `colors:` section: how command-line tools colour their output.
///
/// This is plain data on purpose. The crate does not know what a role or a style
/// is (`shesh-shell` does, and this crate must not depend on it): it hands up the
/// words as written, and the tool that owns the vocabulary checks them and names
/// the key it does not accept. Nothing is validated here except the shape.
///
/// ```yaml
/// colors:
///   enabled: auto        # the tool decides what it accepts
///   highlight: bright-cyan
///   error: bold red
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColorSettings {
    /// The value of the `enabled` key, trimmed, or `None` when the key is absent.
    pub enabled: Option<String>,
    /// Every other key as `(role name, style words)`, in file order, values trimmed.
    /// A blank value stays blank: it is the tool's to refuse.
    pub roles: Vec<(String, String)>,
}

/// The `storage:` section: where the suite's data folders live.
///
/// Every field is optional, and an absent one means "carry on doing whatever
/// you did before this section existed" — each application keeps its own
/// fallback. That is what lets this be added without changing what a machine
/// with no settings file does.
///
/// The folders are named separately, and deliberately never share a
/// default. `beriah-team` keeps rosters, the one place in the suite that maps a
/// participant slot to a real person; `beriah-support` keeps reports, which
/// travel to an outside support team; Pakad keeps its own configuration,
/// progress and review history. Putting them in one repository would
/// undo, in a single line of configuration, the separation the crates are
/// structured to guarantee.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageSettings {
    /// Where `beriah-support` writes reports.
    pub reports: FolderSettings,
    /// Where `beriah-team` reads and writes rosters.
    pub teams: FolderSettings,
    /// Where Pakad keeps its configuration, progress and review history.
    pub pakad: FolderSettings,
}

/// One data folder: where it is, and where it shares to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FolderSettings {
    /// The folder itself.
    ///
    /// A leading `~/` is expanded against the user's home directory, because a
    /// settings file that has to spell out `/Users/somebody` is a settings file
    /// that cannot be copied to another machine.
    pub path: Option<PathBuf>,

    /// The URL the folder's git remote should point at.
    ///
    /// Declaring this makes the application *configure* the remote — see
    /// `one_git::Repository::ensure_remote`. Absent means the folder's existing
    /// git configuration is left exactly as it is, which is what every machine
    /// without this key keeps doing.
    pub remote: Option<String>,
}

/// The `features:` section: capabilities a deployment opts into.
///
/// Every flag here defaults to **off**. A feature switch exists so that a
/// capability can be shipped without being turned on, and a default of `true`
/// would defeat the purpose — the file would have to opt *out* of something the
/// user never asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeatureSettings {
    /// Whether a frontend offers the process for team members leaving:
    /// requesting a departure, confirming it, and bringing somebody back.
    ///
    /// Off does not hide people who have already departed — a roster is still
    /// read and displayed in full, and an entry marked inactive still resolves
    /// and still shows where its work went. What is switched off is the ability
    /// to *change* any of that from the window.
    pub departures: bool,
}

/// The `interface:` section: preferences every frontend honours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceSettings {
    /// The base size of *body* text, in points.
    ///
    /// Frontends scale their entire text hierarchy by the factor this implies
    /// rather than setting every style to this size, so headings stay
    /// proportionally larger.
    pub font_size: i64,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            font_size: DEFAULT_FONT_SIZE,
        }
    }
}

impl InterfaceSettings {
    /// The factor a frontend should multiply its text styles by, given the size
    /// its toolkit currently uses for body text.
    ///
    /// Frontends *scale* rather than assign because a graphical toolkit defines
    /// its heading, button, small, and monospace sizes relative to body text.
    /// Assigning [`Self::font_size`] to each style would flatten that hierarchy
    /// into one size; multiplying preserves it.
    ///
    /// The arithmetic lives here, next to the setting, so both frontends agree
    /// on it and it can be tested without a window. Only the handful of lines
    /// that reach into the toolkit's style live in the applications.
    ///
    /// A `current_body_size` that is zero or not finite yields `1.0` — leaving
    /// the toolkit's own sizing untouched is the only safe answer when there is
    /// nothing meaningful to scale against.
    pub fn scale_factor_from(&self, current_body_size: f32) -> f32 {
        if !current_body_size.is_finite() || current_body_size <= 0.0 {
            return 1.0;
        }
        self.font_size as f32 / current_body_size
    }
}

/// A failure while reading or interpreting the settings file.
#[derive(Debug, Error)]
pub enum SettingsError {
    /// The file exists but could not be read from disk.
    #[error("could not read the settings file {path}: {source}")]
    Unreadable {
        /// The path that failed to read.
        path: PathBuf,
        /// The underlying input/output failure.
        #[source]
        source: std::io::Error,
    },

    /// The file is not well-formed SYON.
    #[error("the settings file {path} is not valid SYON: {source}")]
    Malformed {
        /// The offending file.
        path: PathBuf,
        /// The parser's complaint, which carries the line number.
        #[source]
        source: syon_parser::SyonError,
    },

    /// A key held the wrong structural kind, for example a scalar where the
    /// schema expects a mapping.
    #[error("the settings key {key:?} should be a {expected}, found a {found}")]
    WrongKind {
        /// The dotted path of the offending key, as the user would write it.
        key: String,
        /// The structural kind the schema expects.
        expected: &'static str,
        /// The structural kind actually present.
        found: &'static str,
    },

    /// A scalar could not be converted to the type the schema declares.
    #[error("the settings key {key:?} is not a valid number: {source}")]
    NotANumber {
        /// The dotted path of the offending key.
        key: String,
        /// The shared converter's complaint.
        #[source]
        source: ScalarError,
    },

    /// A scalar could not be converted to the boolean the schema declares.
    #[error("the settings key {key:?} is not a valid true/false value: {source}")]
    NotABoolean {
        /// The dotted path of the offending key.
        key: String,
        /// The shared converter's complaint.
        #[source]
        source: ScalarError,
    },

    /// A value converted cleanly but falls outside its accepted range.
    #[error("the settings key {key:?} is {value}, outside the accepted range {minimum}–{maximum}")]
    OutOfRange {
        /// The dotted path of the offending key.
        key: String,
        /// The value that was rejected.
        value: i64,
        /// The lowest accepted value.
        minimum: i64,
        /// The highest accepted value.
        maximum: i64,
    },
}

/// Resolves the settings file to read, returning the first candidate that
/// exists on disk.
///
/// In order:
///
/// 1. `$BERIAH_SETTINGS_FILE`, an explicit full path.
/// 2. `$XDG_CONFIG_HOME/beriah/settings.syon`.
/// 3. `$HOME/.config/beriah/settings.syon`.
/// 4. `$HOME/.beriah/settings.syon`.
///
/// Returns `None` when the user has no settings file, which is not a failure.
pub fn settings_path() -> Option<PathBuf> {
    candidate_paths().into_iter().find(|path| path.is_file())
}

/// Every path [`settings_path`] considers, in precedence order, whether or not
/// it exists. Useful for telling the user where a file *would* be read from.
pub fn candidate_paths() -> Vec<PathBuf> {
    candidate_paths_for(SETTINGS_FILE_NAME, SETTINGS_FILE_ENVIRONMENT_VARIABLE)
}

/// Every path a file of the suite's configuration directory could live at, in
/// precedence order, whether or not it exists.
///
/// Shared by every file the suite keeps in that directory — `settings.syon` and
/// [`info::INFO_FILE_NAME`] — so the resolution order is defined once and they
/// can never drift apart.
///
/// `override_environment_variable` names a full path to the file and wins over
/// every directory-derived candidate.
pub fn candidate_paths_for(file_name: &str, override_environment_variable: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(explicit) = non_empty_environment_variable(override_environment_variable) {
        candidates.push(PathBuf::from(explicit));
    }
    if let Some(configuration_root) = non_empty_environment_variable("XDG_CONFIG_HOME") {
        candidates.push(
            PathBuf::from(configuration_root)
                .join(SETTINGS_DIRECTORY_NAME)
                .join(file_name),
        );
    }
    if let Some(home_directory) = non_empty_environment_variable("HOME") {
        let home_directory = PathBuf::from(home_directory);
        candidates.push(
            home_directory
                .join(".config")
                .join(SETTINGS_DIRECTORY_NAME)
                .join(file_name),
        );
        candidates.push(
            home_directory
                .join(format!(".{SETTINGS_DIRECTORY_NAME}"))
                .join(file_name),
        );
    }

    candidates
}

/// Where a file of the suite's configuration directory should be *written* when
/// none exists yet: the `$XDG_CONFIG_HOME`/`$HOME/.config` location.
///
/// Writing always picks this one rather than the first existing candidate,
/// because the alternatives are read-compatibility paths, not places to start
/// putting new files.
pub fn write_path_for(file_name: &str) -> Option<PathBuf> {
    if let Some(configuration_root) = non_empty_environment_variable("XDG_CONFIG_HOME") {
        return Some(
            PathBuf::from(configuration_root)
                .join(SETTINGS_DIRECTORY_NAME)
                .join(file_name),
        );
    }
    non_empty_environment_variable("HOME").map(|home_directory| {
        PathBuf::from(home_directory)
            .join(".config")
            .join(SETTINGS_DIRECTORY_NAME)
            .join(file_name)
    })
}

/// The user's home directory, if the environment names one.
fn home_directory() -> Option<PathBuf> {
    non_empty_environment_variable("HOME").map(PathBuf::from)
}

/// Reads the user's settings, falling back to [`Settings::default`] when no
/// settings file exists.
///
/// Only a file that exists and cannot be understood produces an `Err`.
pub fn load() -> Result<Settings, SettingsError> {
    match settings_path() {
        Some(path) => load_from(&path),
        None => Ok(Settings::default()),
    }
}

/// Reads settings from a specific file.
///
/// Unlike [`load`], a missing file here *is* an error: the caller named this
/// path, so its absence is a mistake worth reporting rather than a default.
pub fn load_from(path: &Path) -> Result<Settings, SettingsError> {
    let text = std::fs::read_to_string(path).map_err(|source| SettingsError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    parse_settings(&text).map_err(|error| match error {
        SettingsError::Malformed { source, .. } => SettingsError::Malformed {
            path: path.to_path_buf(),
            source,
        },
        other => other,
    })
}

/// Interprets settings text that has already been read from somewhere.
///
/// Split out from [`load_from`] so the schema can be exercised without a
/// filesystem. The `path` in a [`SettingsError::Malformed`] raised here is
/// empty; `load_from` fills it in.
pub fn parse_settings(text: &str) -> Result<Settings, SettingsError> {
    let document = syon_parser::parse_root(text).map_err(|source| SettingsError::Malformed {
        path: PathBuf::new(),
        source,
    })?;

    let root = match document.as_mapping() {
        Some(mapping) => mapping,
        // An empty document carries no keys, so every key takes its default.
        None => return Ok(Settings::default()),
    };

    let interface = match root.get("interface") {
        None => InterfaceSettings::default(),
        Some(value) => {
            let mapping = value.as_mapping().ok_or_else(|| SettingsError::WrongKind {
                key: "interface".to_string(),
                expected: "mapping",
                found: value.kind_name(),
            })?;
            read_interface(mapping)?
        }
    };

    let features = match root.get("features") {
        None => FeatureSettings::default(),
        Some(value) => {
            let mapping = value.as_mapping().ok_or_else(|| SettingsError::WrongKind {
                key: "features".to_string(),
                expected: "mapping",
                found: value.kind_name(),
            })?;
            read_features(mapping)?
        }
    };

    let storage = match root.get("storage") {
        None => StorageSettings::default(),
        Some(value) => {
            let mapping = value.as_mapping().ok_or_else(|| SettingsError::WrongKind {
                key: "storage".to_string(),
                expected: "mapping",
                found: value.kind_name(),
            })?;
            read_storage(mapping)?
        }
    };

    let colors = match root.get("colors") {
        None => ColorSettings::default(),
        Some(value) => read_colors(value)?,
    };

    Ok(Settings {
        interface,
        features,
        storage,
        colors,
    })
}

/// Reads the `colors:` section into plain `(role, style words)` pairs.
///
/// `colors:` with nothing under it is the same as no section. A value that is
/// not a scalar is an error naming its key (`colors.error`): a style is a few
/// words, and a nested mapping means the writer expected something else.
fn read_colors(value: &syon_parser::Value) -> Result<ColorSettings, SettingsError> {
    let mapping = match value.as_mapping() {
        Some(mapping) => mapping,
        None if value.as_scalar().is_some_and(|text| text.trim().is_empty()) => {
            return Ok(ColorSettings::default());
        }
        None => {
            return Err(SettingsError::WrongKind {
                key: "colors".to_string(),
                expected: "mapping",
                found: value.kind_name(),
            });
        }
    };

    let mut colors = ColorSettings::default();
    for (key, value) in mapping.iter() {
        let raw = value.as_scalar().ok_or_else(|| SettingsError::WrongKind {
            key: format!("colors.{key}"),
            expected: "scalar",
            found: value.kind_name(),
        })?;
        let words = raw.trim().to_string();
        if key == "enabled" {
            colors.enabled = Some(words);
        } else {
            colors.roles.push((key.to_string(), words));
        }
    }
    Ok(colors)
}

/// Reads the `storage:` mapping, leaving absent folders unconfigured.
fn read_storage(mapping: Mapping<'_>) -> Result<StorageSettings, SettingsError> {
    Ok(StorageSettings {
        reports: read_folder(mapping, "reports")?,
        teams: read_folder(mapping, "teams")?,
        pakad: read_folder(mapping, "pakad")?,
    })
}

/// Reads one folder's `path` and `remote`.
///
/// A folder key that is present but empty is not an error — somebody sketching
/// a settings file gets to leave a section half-written — but a key that holds
/// the wrong *kind* is, because `path:` with a nested mapping under it means the
/// writer expected something this schema does not offer.
fn read_folder(storage: Mapping<'_>, folder: &str) -> Result<FolderSettings, SettingsError> {
    let Some(value) = storage.get(folder) else {
        return Ok(FolderSettings::default());
    };
    let mapping = value.as_mapping().ok_or_else(|| SettingsError::WrongKind {
        key: format!("storage.{folder}"),
        expected: "mapping",
        found: value.kind_name(),
    })?;

    Ok(FolderSettings {
        path: read_text(mapping, &format!("storage.{folder}.path"), "path")?
            .map(|text| expand_home(&text)),
        remote: read_text(mapping, &format!("storage.{folder}.remote"), "remote")?,
    })
}

/// Reads an optional scalar as trimmed text, treating an empty value as absent.
///
/// Blank is absent rather than an error because SYON spells "a key with nothing
/// after it" as an empty scalar, and `path:` left blank in a half-written file
/// means the same thing as no `path:` at all.
fn read_text(
    mapping: Mapping<'_>,
    key: &str,
    field: &str,
) -> Result<Option<String>, SettingsError> {
    let Some(value) = mapping.get(field) else {
        return Ok(None);
    };
    let raw = value.as_scalar().ok_or_else(|| SettingsError::WrongKind {
        key: key.to_string(),
        expected: "scalar",
        found: value.kind_name(),
    })?;

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(trimmed.to_string()))
}

/// Expands a leading `~/` against the user's home directory.
///
/// Only a leading `~/` — and only when a home directory is actually known. A
/// bare `~user/…` is left alone rather than guessed at: this crate cannot know
/// where another account's home lives, and inventing a path would turn a
/// question into a wrong answer.
fn expand_home(text: &str) -> PathBuf {
    let Some(rest) = text.strip_prefix("~/") else {
        return PathBuf::from(text);
    };
    match home_directory() {
        Some(home) => home.join(rest),
        None => PathBuf::from(text),
    }
}

/// Reads the `features:` mapping, leaving absent flags switched off.
///
/// A flag that is present but not a boolean is an error rather than a silent
/// `false`: somebody who writes `departures: yes` meant to switch something on,
/// and quietly leaving it off is the one outcome that teaches them nothing.
fn read_features(mapping: Mapping<'_>) -> Result<FeatureSettings, SettingsError> {
    let defaults = FeatureSettings::default();

    let departures = match mapping.get("departures") {
        None => defaults.departures,
        Some(value) => {
            let raw = value.as_scalar().ok_or_else(|| SettingsError::WrongKind {
                key: "features.departures".to_string(),
                expected: "scalar",
                found: value.kind_name(),
            })?;
            read_boolean(raw).map_err(|source| SettingsError::NotABoolean {
                key: "features.departures".to_string(),
                source,
            })?
        }
    };

    Ok(FeatureSettings { departures })
}

/// Reads the `interface:` mapping, leaving absent keys at their defaults.
fn read_interface(mapping: Mapping<'_>) -> Result<InterfaceSettings, SettingsError> {
    let defaults = InterfaceSettings::default();

    let font_size = match mapping.get("font_size") {
        None => defaults.font_size,
        Some(value) => {
            let raw = value.as_scalar().ok_or_else(|| SettingsError::WrongKind {
                key: "interface.font_size".to_string(),
                expected: "scalar",
                found: value.kind_name(),
            })?;
            let size = read_integer(raw).map_err(|source| SettingsError::NotANumber {
                key: "interface.font_size".to_string(),
                source,
            })?;
            if !FONT_SIZE_RANGE.contains(&size) {
                return Err(SettingsError::OutOfRange {
                    key: "interface.font_size".to_string(),
                    value: size,
                    minimum: *FONT_SIZE_RANGE.start(),
                    maximum: *FONT_SIZE_RANGE.end(),
                });
            }
            size
        }
    };

    Ok(InterfaceSettings { font_size })
}

/// Reads an environment variable, treating an empty value as unset.
///
/// An exported-but-empty `XDG_CONFIG_HOME` is common enough that honouring it
/// literally would build the nonsense path `beriah/settings.syon` relative to
/// the working directory.
fn non_empty_environment_variable(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole compatibility promise of the section: a machine with no
    /// `storage:` key behaves exactly as it did before the key existed.
    #[test]
    fn an_absent_storage_section_configures_nothing() {
        let settings = parse_settings("interface:\n  font_size: 20\n").expect("valid");
        assert_eq!(settings.storage, StorageSettings::default());
        assert_eq!(settings.storage.reports.path, None);
        assert_eq!(settings.storage.reports.remote, None);
        assert_eq!(settings.storage.teams.path, None);
        assert_eq!(settings.storage.teams.remote, None);
    }

    #[test]
    fn storage_pakad_reads_a_path_and_a_remote() {
        let settings = parse_settings(
            "storage:\n  pakad:\n    path: /data/pakad\n    remote: git@example.org:me/pakad.git\n",
        )
        .unwrap();
        assert_eq!(
            settings.storage.pakad.path,
            Some(PathBuf::from("/data/pakad"))
        );
        assert_eq!(
            settings.storage.pakad.remote.as_deref(),
            Some("git@example.org:me/pakad.git")
        );
    }

    #[test]
    fn an_absent_storage_pakad_is_the_default() {
        assert_eq!(
            parse_settings("storage:\n  teams:\n    path: /t\n")
                .unwrap()
                .storage
                .pakad,
            FolderSettings::default()
        );
    }

    #[test]
    fn a_folder_and_its_remote_are_read() {
        let settings = parse_settings(
            "storage:\n  \
             reports:\n    \
             path: /srv/beriah/reports\n    \
             remote: git@example.invalid:beriah/reports.git\n  \
             teams:\n    \
             path: /srv/beriah/teams\n    \
             remote: git@example.invalid:beriah/teams.git\n",
        )
        .expect("valid");

        assert_eq!(
            settings.storage.reports.path,
            Some(PathBuf::from("/srv/beriah/reports"))
        );
        assert_eq!(
            settings.storage.reports.remote.as_deref(),
            Some("git@example.invalid:beriah/reports.git")
        );
        assert_eq!(
            settings.storage.teams.path,
            Some(PathBuf::from("/srv/beriah/teams"))
        );
        assert_eq!(
            settings.storage.teams.remote.as_deref(),
            Some("git@example.invalid:beriah/teams.git")
        );
    }

    /// Each key stands on its own: naming a folder without a remote is an
    /// ordinary thing to want, and so is the reverse.
    #[test]
    fn a_folder_may_declare_a_path_without_a_remote() {
        let settings =
            parse_settings("storage:\n  reports:\n    path: /srv/reports\n").expect("valid");
        assert_eq!(
            settings.storage.reports.path,
            Some(PathBuf::from("/srv/reports"))
        );
        assert_eq!(settings.storage.reports.remote, None);
    }

    /// A settings file that has to spell out `/Users/somebody` cannot be copied
    /// to another machine, which is most of the point of having one file.
    #[test]
    fn a_leading_tilde_is_expanded_against_the_home_directory() {
        let expanded = expand_home("~/beriah/reports");
        match home_directory() {
            Some(home) => assert_eq!(expanded, home.join("beriah/reports")),
            None => assert_eq!(expanded, PathBuf::from("~/beriah/reports")),
        }
    }

    /// Only a leading `~/`. Another account's home is not this crate's to guess.
    #[test]
    fn a_tilde_anywhere_else_is_left_alone() {
        assert_eq!(
            expand_home("/srv/~/reports"),
            PathBuf::from("/srv/~/reports")
        );
        assert_eq!(
            expand_home("~colleague/reports"),
            PathBuf::from("~colleague/reports")
        );
    }

    /// A half-written file is a normal thing to have on disk.
    #[test]
    fn a_blank_value_reads_as_absent_rather_than_as_an_empty_path() {
        let settings = parse_settings("storage:\n  reports:\n    path:\n").expect("valid");
        assert_eq!(settings.storage.reports.path, None);
    }

    #[test]
    fn a_storage_section_of_the_wrong_kind_is_rejected() {
        let error = parse_settings("storage: nonsense\n").expect_err("a scalar is not a mapping");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, .. } if key == "storage"),
            "got {error}"
        );
    }

    #[test]
    fn a_folder_key_of_the_wrong_kind_is_named_in_full() {
        let error =
            parse_settings("storage:\n  reports: /srv/reports\n").expect_err("not a mapping");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, .. } if key == "storage.reports"),
            "got {error}"
        );
    }

    #[test]
    fn a_folder_key_pakad_of_the_wrong_kind_is_rejected() {
        let error = parse_settings("storage:\n  pakad: /data/pakad\n").expect_err("not a mapping");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, .. } if key == "storage.pakad"),
            "got {error}"
        );
    }

    /// Forwards compatibility, the same promise `features:` makes.
    #[test]
    fn an_unknown_storage_key_is_ignored() {
        let settings =
            parse_settings("storage:\n  archives:\n    path: /srv/archives\n").expect("valid");
        assert_eq!(settings.storage, StorageSettings::default());
    }

    #[test]
    fn every_feature_is_off_by_default() {
        // The whole point of a switch: shipped dark until somebody asks for it.
        assert!(!Settings::default().features.departures);
        assert!(!parse_settings("").expect("valid").features.departures);
        assert!(
            !parse_settings("interface:\n  font_size: 20\n")
                .expect("valid")
                .features
                .departures
        );
    }

    #[test]
    fn a_feature_is_switched_on_from_the_features_section() {
        let settings = parse_settings("features:\n  departures: true\n").expect("valid");
        assert!(settings.features.departures);
    }

    #[test]
    fn a_feature_can_be_switched_off_explicitly() {
        let settings = parse_settings("features:\n  departures: false\n").expect("valid");
        assert!(!settings.features.departures);
    }

    #[test]
    fn an_unknown_feature_key_is_ignored() {
        // Forwards compatibility: an older build must not refuse a file that
        // mentions a switch it has never heard of.
        let settings = parse_settings("features:\n  time_travel: true\n").expect("valid");
        assert!(!settings.features.departures);
    }

    /// A flag meant to switch something *on* must not fail quietly to off.
    #[test]
    fn a_non_boolean_feature_is_rejected() {
        let error = parse_settings("features:\n  departures: yes\n")
            .expect_err("`yes` is not how this file spells true");
        assert!(
            matches!(error, SettingsError::NotABoolean { ref key, .. } if key == "features.departures"),
            "expected a boolean complaint naming the key, got {error}"
        );
    }

    #[test]
    fn a_features_section_of_the_wrong_kind_is_rejected() {
        let error = parse_settings("features: true\n").expect_err("`features` must be a mapping");
        assert!(
            matches!(error, SettingsError::WrongKind { ref key, .. } if key == "features"),
            "got {error}"
        );
    }

    #[test]
    fn an_empty_document_yields_the_defaults() {
        let settings = parse_settings("").expect("an empty document is valid");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn a_document_without_an_interface_section_yields_the_defaults() {
        let settings = parse_settings("version: 1\n").expect("valid");
        assert_eq!(settings.interface.font_size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn the_font_size_is_read_from_the_interface_section() {
        let settings = parse_settings("interface:\n  font_size: 24\n").expect("valid");
        assert_eq!(settings.interface.font_size, 24);
    }

    #[test]
    fn an_absent_font_size_leaves_the_default_in_place() {
        let settings = parse_settings("interface:\n  theme: dark\n").expect("valid");
        assert_eq!(settings.interface.font_size, DEFAULT_FONT_SIZE);
    }

    #[test]
    fn a_non_numeric_font_size_is_rejected_by_name() {
        let error = parse_settings("interface:\n  font_size: large\n")
            .expect_err("`large` is not an integer");
        assert!(
            matches!(&error, SettingsError::NotANumber { key, .. } if key == "interface.font_size"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_font_size_outside_the_range_is_rejected() {
        let error =
            parse_settings("interface:\n  font_size: 0\n").expect_err("0 points is unreadable");
        assert!(
            matches!(&error, SettingsError::OutOfRange { value, .. } if *value == 0),
            "unexpected error: {error}"
        );

        let error = parse_settings("interface:\n  font_size: 500\n")
            .expect_err("500 points fits nothing on screen");
        assert!(
            matches!(&error, SettingsError::OutOfRange { value, .. } if *value == 500),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn both_ends_of_the_range_are_accepted() {
        for size in [*FONT_SIZE_RANGE.start(), *FONT_SIZE_RANGE.end()] {
            let settings =
                parse_settings(&format!("interface:\n  font_size: {size}\n")).expect("in range");
            assert_eq!(settings.interface.font_size, size);
        }
    }

    #[test]
    fn an_interface_section_of_the_wrong_kind_is_rejected() {
        let error = parse_settings("interface: 18\n").expect_err("`interface` must be a mapping");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, .. } if key == "interface"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn a_font_size_of_the_wrong_kind_is_rejected() {
        let error = parse_settings("interface:\n  font_size:\n    - 18\n")
            .expect_err("`font_size` must be a scalar");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, found, .. }
                if key == "interface.font_size" && *found == "sequence"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn malformed_syon_is_reported_as_malformed() {
        // Duplicate keys are a hard error in SYON.
        let error = parse_settings("interface:\n  font_size: 18\n  font_size: 20\n")
            .expect_err("duplicate keys are rejected");
        assert!(
            matches!(error, SettingsError::Malformed { .. }),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn the_scale_factor_takes_body_text_to_the_configured_size() {
        let interface = InterfaceSettings { font_size: 25 };
        let factor = interface.scale_factor_from(12.5);
        assert!((factor - 2.0).abs() < f32::EPSILON, "factor was {factor}");
        assert!((12.5 * factor - 25.0).abs() < 0.001);
    }

    #[test]
    fn a_body_size_that_cannot_be_scaled_against_leaves_the_toolkit_alone() {
        let interface = InterfaceSettings::default();
        for impossible in [0.0, -12.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                interface.scale_factor_from(impossible),
                1.0,
                "a body size of {impossible} should not change anything"
            );
        }
    }

    #[test]
    fn load_from_names_the_file_it_could_not_read() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let missing = directory.path().join("settings.syon");
        let error = load_from(&missing).expect_err("the file does not exist");
        assert!(
            matches!(&error, SettingsError::Unreadable { path, .. } if path == &missing),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_from_reads_a_real_file_and_reports_its_path_on_a_parse_failure() {
        let directory = tempfile::tempdir().expect("a temporary directory");

        let good = directory.path().join("good.syon");
        std::fs::write(&good, "interface:\n  font_size: 22\n").expect("write");
        assert_eq!(load_from(&good).expect("valid").interface.font_size, 22);

        let bad = directory.path().join("bad.syon");
        std::fs::write(&bad, "interface:\n  font_size: 18\n  font_size: 20\n").expect("write");
        let error = load_from(&bad).expect_err("duplicate keys are rejected");
        assert!(
            matches!(&error, SettingsError::Malformed { path, .. } if path == &bad),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn an_absent_colors_section_configures_nothing() {
        let settings = parse_settings("interface:\n  font_size: 20\n").expect("valid");
        assert_eq!(settings.colors, ColorSettings::default());
        assert_eq!(settings.colors.enabled, None);
        assert!(settings.colors.roles.is_empty());
        assert_eq!(
            parse_settings("colors:\n").expect("valid").colors,
            ColorSettings::default()
        );
    }

    #[test]
    fn colors_are_read_as_role_and_style_words_in_file_order() {
        let settings = parse_settings(
            "colors:\n  enabled: always\n  highlight: bright-cyan\n  error: bold red\n  made_up: whatever words\n",
        )
        .expect("valid");
        assert_eq!(settings.colors.enabled.as_deref(), Some("always"));
        assert_eq!(
            settings.colors.roles,
            vec![
                ("highlight".to_string(), "bright-cyan".to_string()),
                ("error".to_string(), "bold red".to_string()),
                ("made_up".to_string(), "whatever words".to_string()),
            ],
            "no role or style is judged here: the tool that owns the words does that"
        );
    }

    #[test]
    fn a_blank_color_value_is_handed_up_blank() {
        let settings = parse_settings("colors:\n  error:\n  enabled:\n").expect("valid");
        assert_eq!(settings.colors.enabled.as_deref(), Some(""));
        assert_eq!(
            settings.colors.roles,
            vec![("error".to_string(), String::new())]
        );
    }

    #[test]
    fn a_colors_section_of_the_wrong_kind_is_rejected() {
        let error = parse_settings("colors: red\n").expect_err("`colors` must be a mapping");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, .. } if key == "colors"),
            "got {error}"
        );
    }

    #[test]
    fn a_color_value_of_the_wrong_kind_is_named_in_full() {
        let error =
            parse_settings("colors:\n  error:\n    nested: x\n").expect_err("a style is a scalar");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, found, .. }
                if key == "colors.error" && *found == "mapping"),
            "got {error}"
        );
    }

    /// The other direction of the promise: a `colors:` block does not disturb the
    /// sections that were there before it.
    #[test]
    fn the_colors_section_leaves_the_other_sections_alone() {
        let settings = parse_settings(
            "colors:\n  error: bold red\ninterface:\n  font_size: 20\nfeatures:\n  departures: true\n",
        )
        .expect("valid");
        assert_eq!(settings.interface.font_size, 20);
        assert!(settings.features.departures);
    }

    #[test]
    fn load_from_reads_the_colors_of_a_real_file() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let file = directory.path().join("settings.syon");
        std::fs::write(&file, "colors:\n  highlight: green\n").expect("write");
        assert_eq!(
            load_from(&file).expect("valid").colors.roles,
            vec![("highlight".to_string(), "green".to_string())]
        );
    }
}
