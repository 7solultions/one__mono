//! Who the person at this machine is, as `info.syon` in the suite's
//! configuration directory.
//!
//! ```yaml
//! # ~/.config/beriah/info.syon
//! company-id: c001
//! team-id: p1
//! ```
//!
//! ## Why this is identity but not personal data
//!
//! The file holds a **company id** and a **participant slot** — never a name,
//! an email, or anything else that identifies a human being directly. That is
//! not an oversight, it is the suite's privacy model showing through: a slot is
//! the only identity the reporter is allowed to know, and resolving a slot to a
//! person needs a roster the reporter does not depend on.
//!
//! So this module hands back two opaque strings. It does not know what a
//! company id or a slot *is*, does not validate their shape, and cannot look
//! either of them up. Callers that own those types — `beriah-team-core` — parse
//! them; keeping that knowledge out of here is what lets this crate sit in the
//! foundational layer without dragging the team model along with it.
//!
//! ## Both keys are optional
//!
//! A half-filled file is a normal intermediate state: an application that finds
//! one key missing asks for it and offers to [`save_to`] the result. There is no
//! error for "incomplete", only for "unreadable".

use std::path::{Path, PathBuf};

use syon_parser::{emit, EntriesMut, Mapping, MappingEntry, Value};

use crate::{candidate_paths_for, write_path_for, SettingsError};

/// The file name looked for inside the suite's configuration directory.
pub const INFO_FILE_NAME: &str = "info.syon";

/// The environment variable holding an explicit path to an info file. It names
/// the *file*, so tests and one-off runs need not touch the real one.
pub const INFO_FILE_ENVIRONMENT_VARIABLE: &str = "BERIAH_INFO_FILE";

/// The key naming the company the person belongs to.
pub const COMPANY_ID_KEY: &str = "company-id";

/// The key naming the person's participant slot within that company.
pub const TEAM_ID_KEY: &str = "team-id";

/// The identity of the person at this machine.
///
/// Both fields are raw strings on purpose — see the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// The company id, as written in the file (for example `c001`).
    pub company_id: Option<String>,
    /// The participant slot, as written in the file (for example `p1`).
    pub team_id: Option<String>,
}

impl Info {
    /// Whether both keys are present, so an application need not ask.
    pub fn is_complete(&self) -> bool {
        self.company_id.is_some() && self.team_id.is_some()
    }
}

/// Resolves the info file to read, returning the first candidate that exists.
///
/// The same order as [`crate::settings_path`], over [`INFO_FILE_NAME`].
pub fn info_path() -> Option<PathBuf> {
    candidate_paths_for(INFO_FILE_NAME, INFO_FILE_ENVIRONMENT_VARIABLE)
        .into_iter()
        .find(|path| path.is_file())
}

/// Where a new info file should be written when none exists yet.
///
/// `None` only when neither `$XDG_CONFIG_HOME` nor `$HOME` is set, which leaves
/// nowhere sensible to put it.
pub fn info_write_path() -> Option<PathBuf> {
    write_path_for(INFO_FILE_NAME)
}

/// Reads the person's identity, yielding an empty [`Info`] when no file exists.
///
/// A missing file is the first-run case, not a failure. A file that exists but
/// cannot be parsed *is* reported, for the same reason settings are: silently
/// treating a typo as "no identity" would send the person round the login
/// dialog again with no explanation.
pub fn load() -> Result<Info, SettingsError> {
    match info_path() {
        Some(path) => load_from(&path),
        None => Ok(Info::default()),
    }
}

/// Reads identity from a specific file. A missing file here *is* an error,
/// because the caller named the path.
pub fn load_from(path: &Path) -> Result<Info, SettingsError> {
    let text = std::fs::read_to_string(path).map_err(|source| SettingsError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    parse_info(&text).map_err(|error| match error {
        SettingsError::Malformed { source, .. } => SettingsError::Malformed {
            path: path.to_path_buf(),
            source,
        },
        other => other,
    })
}

/// Interprets info text that has already been read from somewhere.
pub fn parse_info(text: &str) -> Result<Info, SettingsError> {
    let document = syon_parser::parse_root(text).map_err(|source| SettingsError::Malformed {
        path: PathBuf::new(),
        source,
    })?;

    let Some(root) = document.as_mapping() else {
        return Ok(Info::default());
    };

    Ok(Info {
        company_id: read_optional_scalar(root, COMPANY_ID_KEY)?,
        team_id: read_optional_scalar(root, TEAM_ID_KEY)?,
    })
}

/// Reads one optional scalar key, treating blank text as absent.
///
/// A key left empty (`team-id:`) reads as "not answered yet" rather than as an
/// identity of `""`, which nothing downstream could resolve.
fn read_optional_scalar(mapping: Mapping<'_>, key: &str) -> Result<Option<String>, SettingsError> {
    let Some(value) = mapping.get(key) else {
        return Ok(None);
    };
    let raw = value.as_scalar().ok_or_else(|| SettingsError::WrongKind {
        key: key.to_string(),
        expected: "scalar",
        found: value.kind_name(),
    })?;

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

/// Renders an [`Info`] as SYON text.
///
/// Absent keys are *omitted* rather than written empty, which is the emitter's
/// own rule for anything with no value to spell.
pub fn emit_info(info: &Info) -> String {
    let mut root: Vec<MappingEntry> = Vec::new();
    if let Some(company_id) = &info.company_id {
        root.set(COMPANY_ID_KEY, Value::scalar(company_id.clone()));
    }
    if let Some(team_id) = &info.team_id {
        root.set(TEAM_ID_KEY, Value::scalar(team_id.clone()));
    }

    emit(&Value::mapping(root))
}

/// Writes an [`Info`] to a specific file, creating parent directories.
pub fn save_to(path: &Path, info: &Info) -> Result<(), SettingsError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SettingsError::Unreadable {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, emit_info(info)).map_err(|source| SettingsError::Unreadable {
        path: path.to_path_buf(),
        source,
    })
}

/// Writes an [`Info`] to wherever the suite keeps its configuration.
///
/// Prefers the file already in use, so answering a login dialog updates the
/// file the person actually has rather than shadowing it with a second one.
pub fn save(info: &Info) -> Result<PathBuf, SettingsError> {
    let path = info_path()
        .or_else(info_write_path)
        .ok_or_else(|| SettingsError::Unreadable {
            path: PathBuf::from(INFO_FILE_NAME),
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "neither XDG_CONFIG_HOME nor HOME is set, so there is nowhere to write it",
            ),
        })?;
    save_to(&path, info)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_yields_no_identity() {
        let info = parse_info("").expect("an empty document is valid");
        assert_eq!(info, Info::default());
        assert!(!info.is_complete());
    }

    #[test]
    fn both_keys_are_read() {
        let info = parse_info("company-id: c001\nteam-id: p1\n").expect("valid");
        assert_eq!(info.company_id.as_deref(), Some("c001"));
        assert_eq!(info.team_id.as_deref(), Some("p1"));
        assert!(info.is_complete());
    }

    #[test]
    fn a_half_filled_file_is_valid_but_incomplete() {
        let info = parse_info("company-id: c001\n").expect("valid");
        assert_eq!(info.company_id.as_deref(), Some("c001"));
        assert_eq!(info.team_id, None);
        assert!(!info.is_complete());
    }

    #[test]
    fn a_blank_value_reads_as_unanswered() {
        let info = parse_info("company-id: c001\nteam-id:\n").expect("valid");
        assert_eq!(info.team_id, None);
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let info = parse_info("company-id:   c001  \n").expect("valid");
        assert_eq!(info.company_id.as_deref(), Some("c001"));
    }

    #[test]
    fn a_key_of_the_wrong_kind_is_rejected_by_name() {
        let error = parse_info("team-id:\n  - p1\n").expect_err("a slot is not a list");
        assert!(
            matches!(&error, SettingsError::WrongKind { key, .. } if key == TEAM_ID_KEY),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn unknown_keys_are_ignored_rather_than_refused() {
        // Forward compatibility: a newer application's key must not stop an
        // older one from reading the identity it does understand.
        let info =
            parse_info("company-id: c001\nteam-id: p1\nfavourite-colour: blue\n").expect("valid");
        assert!(info.is_complete());
    }

    #[test]
    fn what_is_emitted_parses_back_to_the_same_identity() {
        let info = Info {
            company_id: Some("c001".to_string()),
            team_id: Some("p1".to_string()),
        };
        assert_eq!(parse_info(&emit_info(&info)).expect("valid"), info);
    }

    #[test]
    fn an_absent_key_is_omitted_rather_than_written_empty() {
        let info = Info {
            company_id: Some("c001".to_string()),
            team_id: None,
        };
        let text = emit_info(&info);
        assert!(text.contains("company-id: c001"), "got: {text:?}");
        assert!(!text.contains(TEAM_ID_KEY), "got: {text:?}");
    }

    #[test]
    fn saving_creates_the_directory_and_round_trips() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("nested").join(INFO_FILE_NAME);

        let info = Info {
            company_id: Some("c002".to_string()),
            team_id: Some("p42".to_string()),
        };
        save_to(&path, &info).expect("save");

        assert_eq!(load_from(&path).expect("load"), info);
    }
}
