//! Sharing a Beriah data folder with colleagues through a git repository.
//!
//! Two applications in the suite keep their data as files a person is expected
//! to share: `beriah-team` keeps rosters, shared with colleagues, and
//! `beriah-support` keeps reports, shared with colleagues *and* an outside
//! support team. Both formats were already built for this — byte-identical
//! re-emission, per-file change tracking, second-precision timestamps — so what
//! was missing was the plumbing, not the file format.
//!
//! ## Why this shells out to `git`
//!
//! Every command runs the user's own `git` binary. The hard part of this
//! feature is not the porcelain, it is **authentication**: SSH agents,
//! credential helpers, corporate proxies, `includeIf` config, signing keys.
//! Shelling out inherits whatever already works on that machine, and when
//! something fails the user can run the same command by hand and see the same
//! error. An in-process library would have to re-implement each of those, and
//! would fail on exactly the laptops where this matters most.
//!
//! Two environment variables are set on every invocation:
//!
//! * `GIT_TERMINAL_PROMPT=0` — git must never stop and wait for a password.
//!   A graphical application has no terminal to answer it, so a prompt would
//!   hang the process forever rather than fail.
//! * `GIT_OPTIONAL_LOCKS=0` — a read-only query must not take the index lock
//!   and fight with a terminal the user has open on the same folder.
//!
//! ## Why it refuses rather than merges
//!
//! [`Repository::sync`] pulls with `--ff-only`. If the remote has moved in a way
//! that cannot fast-forward, it stops and says so, leaving the working tree
//! exactly as it found it.
//!
//! That is deliberate. One of these repositories holds personal data and the
//! other holds other people's incident reports; an automatic merge would be
//! this crate inventing a resolution nobody reviewed, in files where being
//! quietly wrong is expensive. Git already has good tools for resolving a
//! divergence, the user already has them installed, and the honest thing is to
//! hand the problem over with the working tree untouched.
//!
//! ## It only ever touches its own folder
//!
//! A data folder is usually a *subfolder* of a much larger repository. Every
//! status query, every `add`, and every `commit` is confined to that folder by
//! pathspec, so a share cannot pick up unrelated work sitting elsewhere in the
//! tree. This is not a nicety: an early version staged the whole repository and
//! committed a colleague's in-flight refactor along with a roster.
//!
//! ## Looking is separate from sharing
//!
//! [`Repository::incoming`] answers "is there anything waiting for me?" without
//! committing or pushing anything. That separation is what makes the question
//! safe to ask on a timer: a frontend can show a count every few minutes, and
//! the user still decides when their own work leaves the machine.
//!
//! It is also the only honest way to count. [`Repository::sync`] must
//! fast-forward on *any* incoming commit, so its merge decision looks at the
//! whole repository — but the number a person is shown has to be scoped to the
//! data folder, or a mono-repo with the data inside it reports every unrelated
//! source commit as "changes from your colleagues".
//!
//! ## The remote comes from settings
//!
//! [`Repository::ensure_remote`] points the folder's remote at a declared URL,
//! adding it when there is none and retargeting it when it points elsewhere.
//! The suite's `settings.syon` is where that URL lives, so a deployment
//! configures sharing in one file rather than by talking every user through
//! `git remote add` on their own laptop.
//!
//! This is the one thing in this crate that writes git *configuration* rather
//! than history, and it is deliberately fenced:
//!
//! * **Only when the data folder is the repository root.** A remote belongs to
//!   a whole repository, not to a pathspec, so retargeting one from inside a
//!   subfolder would repoint a repository that merely happens to contain the
//!   data — a reports folder sitting in a source checkout would rewrite that
//!   checkout's `origin`. A subfolder is refused with
//!   [`GitError::NotItsOwnRepository`].
//! * **It still does not clone or `init`.** A folder is either already a
//!   repository or sharing is off for it; creating one is still a deliberate
//!   act performed with the user's own tools.
//!
//! What remains true is the thing worth remembering: whoever writes the
//! settings file decides where this data is pushed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use thiserror::Error;

/// How long a single git invocation may run before it is treated as stuck.
///
/// Not enforced here — a `Command` has no built-in timeout — but recorded as
/// the contract callers should hold themselves to when running [`Repository`]
/// off the main thread. See the crate documentation on `GIT_TERMINAL_PROMPT`
/// for why a hang is unlikely in the first place.
pub const SUGGESTED_TIMEOUT_SECONDS: u64 = 120;

/// Why a git operation could not be carried out.
#[derive(Debug, Error)]
pub enum GitError {
    /// The `git` binary is not on the path.
    #[error("git is not installed, or not on the PATH")]
    GitNotAvailable,

    /// The folder is not inside a git repository.
    #[error("{path} is not inside a git repository")]
    NotARepository {
        /// The folder that was asked about.
        path: String,
    },

    /// The repository has no remote to share through.
    #[error("this repository has no remote named {remote:?} to share through")]
    NoRemote {
        /// The remote that was looked for.
        remote: String,
    },

    /// The local and remote histories have diverged.
    #[error(
        "{remote} has changes that cannot be fast-forwarded onto your copy — \
         resolve it in git, then sync again"
    )]
    Diverged {
        /// The remote involved.
        remote: String,
    },

    /// The working tree holds a conflict from an earlier operation.
    #[error("this repository has unresolved conflicts — finish resolving them, then sync again")]
    Conflicted,

    /// A push was refused because the remote moved in the meantime.
    #[error("{remote} moved while this was running — sync again")]
    PushRejected {
        /// The remote involved.
        remote: String,
    },

    /// A git command exited non-zero for a reason not modelled above.
    #[error("git {command} failed: {message}")]
    CommandFailed {
        /// The subcommand, for the message.
        command: String,
        /// What git said, trimmed.
        message: String,
    },

    /// A remote was declared for a folder that does not own its repository.
    #[error(
        "cannot configure a remote for {folder}: it is a subfolder of the repository at {root}, \
         and a remote belongs to the whole repository rather than to one folder — give this \
         folder its own repository, or drop the remote from your settings"
    )]
    NotItsOwnRepository {
        /// The data folder, relative to the repository root.
        folder: String,
        /// The repository that folder turned out to live in.
        root: String,
    },

    /// The command could not be started at all.
    #[error("could not run git: {0}")]
    Io(#[from] std::io::Error),
}

/// What [`Repository::ensure_remote`] did to the folder's git configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteOutcome {
    /// The remote already pointed at the declared URL. Nothing was written.
    Unchanged,

    /// There was no such remote, and it was added.
    Added {
        /// The URL it now points at.
        url: String,
    },

    /// The remote existed and pointed somewhere else, and was repointed.
    ///
    /// The previous URL is carried so a frontend can *say* what it changed.
    /// Silently moving where somebody's data gets pushed is the one outcome
    /// here that would be worth hiding and must not be.
    Retargeted {
        /// Where it used to point.
        previous: String,
        /// Where it points now.
        url: String,
    },
}

impl RemoteOutcome {
    /// Whether anything was actually written.
    pub fn changed_anything(&self) -> bool {
        !matches!(self, Self::Unchanged)
    }

    /// A one-line summary for a status bar, or `None` when nothing changed.
    pub fn summary(&self) -> Option<String> {
        match self {
            Self::Unchanged => None,
            Self::Added { url } => Some(format!("Sharing is now set up with {url}.")),
            Self::Retargeted { previous, url } => {
                Some(format!("Sharing now goes to {url} instead of {previous}."))
            }
        }
    }
}

/// One file changed relative to the last commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The path, relative to the repository root.
    pub path: PathBuf,
    /// The two-letter status git reported, e.g. `??` or ` M`.
    pub status: String,
}

impl Change {
    /// Whether git considers this file new.
    pub fn is_new(&self) -> bool {
        self.status.starts_with("??") || self.status.starts_with('A')
    }
}

/// What a [`Repository::sync`] actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Commits brought down from the remote.
    pub received: usize,
    /// Files committed and pushed.
    pub shared: Vec<PathBuf>,
}

impl SyncOutcome {
    /// Whether nothing at all moved in either direction.
    pub fn is_quiet(&self) -> bool {
        self.received == 0 && self.shared.is_empty()
    }

    /// A one-line summary for a status bar.
    pub fn summary(&self) -> String {
        match (self.received, self.shared.len()) {
            (0, 0) => "Already up to date with everybody else.".to_string(),
            (received, 0) => format!("Received {received} change(s) from your colleagues."),
            (0, shared) => format!("Shared {shared} file(s) with your colleagues."),
            (received, shared) => {
                format!("Received {received} change(s) and shared {shared} file(s).")
            }
        }
    }
}

/// A git repository holding one of the suite's data folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    root: PathBuf,
    /// The data folder, relative to `root`.
    ///
    /// Everything this crate stages, commits, and reports is confined to it. A
    /// data folder is very often a subfolder of a much larger repository — the
    /// suite's own example rosters live inside the mono-repo — and a share that
    /// staged the whole tree would sweep up work that has nothing to do with
    /// it. It did exactly that once.
    scope: PathBuf,
    remote: String,
}

/// The remote used unless a caller names another.
pub const DEFAULT_REMOTE: &str = "origin";

impl Repository {
    /// Finds the repository containing `path`, if there is one.
    ///
    /// Returns `Ok(None)` rather than an error when the folder is simply not in
    /// a repository: sharing being switched off for a folder is an ordinary
    /// state, not a failure, and every caller would otherwise have to translate
    /// the error back into that.
    pub fn containing(path: impl AsRef<Path>) -> Result<Option<Self>, GitError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }

        let output = match run(path, ["rev-parse", "--show-toplevel"]) {
            Ok(output) => output,
            Err(GitError::GitNotAvailable) => return Err(GitError::GitNotAvailable),
            Err(other) => return Err(other),
        };

        if !output.status.success() {
            // "not a git repository" is the expected failure here.
            return Ok(None);
        }

        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if root.is_empty() {
            return Ok(None);
        }

        let root = PathBuf::from(root);
        // Relative to the root, so it can be handed to git as a pathspec.
        // Falls back to the whole repository only when the folder *is* the
        // root, never as a guess.
        let scope = match (path.canonicalize(), root.canonicalize()) {
            (Ok(folder), Ok(canonical_root)) => folder
                .strip_prefix(&canonical_root)
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            _ => PathBuf::new(),
        };

        Ok(Some(Self {
            root,
            scope,
            remote: DEFAULT_REMOTE.to_string(),
        }))
    }

    /// Uses a remote other than [`DEFAULT_REMOTE`].
    pub fn with_remote(mut self, remote: impl Into<String>) -> Self {
        self.remote = remote.into();
        self
    }

    /// The repository's top-level folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The remote this repository shares through.
    pub fn remote(&self) -> &str {
        &self.remote
    }

    /// The data folder this repository shares, relative to its root.
    ///
    /// Empty when the data folder *is* the repository root.
    pub fn scope(&self) -> &Path {
        &self.scope
    }

    /// The pathspec every staging and status query is confined to.
    fn pathspec(&self) -> String {
        match self.scope.to_str() {
            Some("") | None => ".".to_string(),
            Some(scope) => scope.to_string(),
        }
    }

    /// Whether the repository has the remote it would share through.
    pub fn has_remote(&self) -> Result<bool, GitError> {
        let output = run(&self.root, ["remote"])?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == self.remote))
    }

    /// The URL this repository's remote points at, if the remote exists.
    pub fn remote_url(&self) -> Result<Option<String>, GitError> {
        let output = run(&self.root, ["remote", "get-url", &self.remote])?;
        if !output.status.success() {
            // "No such remote" is the expected failure, and an ordinary state.
            return Ok(None);
        }
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!url.is_empty()).then_some(url))
    }

    /// Points this repository's remote at `url`, adding or retargeting it.
    ///
    /// This is how a deployment configures sharing centrally: the URL lives in
    /// the suite's shared settings file, and the application makes the working
    /// copy match rather than talking every user through `git remote add`.
    ///
    /// Refused with [`GitError::NotItsOwnRepository`] when the data folder is a
    /// *subfolder* of its repository. Everything else in this crate is confined
    /// to the data folder by pathspec, but a remote cannot be: it belongs to the
    /// whole repository. Retargeting one from inside a subfolder would repoint
    /// a repository that merely happens to contain the data — for a reports
    /// folder sitting in a source checkout, that means rewriting the source
    /// checkout's `origin`.
    ///
    /// Still no `clone` and no `init`: a folder outside a repository stays
    /// outside one.
    pub fn ensure_remote(&self, url: &str) -> Result<RemoteOutcome, GitError> {
        if !self.scope.as_os_str().is_empty() {
            return Err(GitError::NotItsOwnRepository {
                folder: self.scope.display().to_string(),
                root: self.root.display().to_string(),
            });
        }

        match self.remote_url()? {
            Some(existing) if existing == url => Ok(RemoteOutcome::Unchanged),
            Some(previous) => {
                checked(
                    run(&self.root, ["remote", "set-url", &self.remote, url])?,
                    "remote set-url",
                )?;
                Ok(RemoteOutcome::Retargeted {
                    previous,
                    url: url.to_string(),
                })
            }
            None => {
                checked(
                    run(&self.root, ["remote", "add", &self.remote, url])?,
                    "remote add",
                )?;
                Ok(RemoteOutcome::Added {
                    url: url.to_string(),
                })
            }
        }
    }

    /// Everything changed in the working tree, relative to the last commit.
    ///
    /// Includes untracked files, because a brand-new roster or report is
    /// exactly the thing a person most expects sharing to pick up.
    pub fn local_changes(&self) -> Result<Vec<Change>, GitError> {
        let output = checked(
            run(
                &self.root,
                ["status", "--porcelain", "--", &self.pathspec()],
            )?,
            "status",
        )?;

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.len() > 3)
            .map(|line| Change {
                status: line[..2].to_string(),
                // Porcelain v1 pads the status to two characters and a space.
                // A rename reads `old -> new`; the new name is what matters.
                path: PathBuf::from(match line[3..].split_once(" -> ") {
                    Some((_old, new)) => new,
                    None => &line[3..],
                }),
            })
            .collect())
    }

    /// Whether the working tree has an unresolved conflict.
    pub fn is_conflicted(&self) -> Result<bool, GitError> {
        Ok(self
            .local_changes()?
            .iter()
            .any(|change| is_conflict_status(&change.status)))
    }

    /// Brings colleagues' changes down, then shares this copy's changes.
    ///
    /// In order: refuse if the tree is already conflicted, fetch, fast-forward,
    /// commit everything changed, push. Any step that cannot be done cleanly
    /// stops the whole thing with the working tree as it was found — see the
    /// crate documentation for why this never merges.
    ///
    /// `message` is the commit subject. It is only used when there is something
    /// to commit.
    pub fn sync(&self, message: &str) -> Result<SyncOutcome, GitError> {
        if !self.has_remote()? {
            return Err(GitError::NoRemote {
                remote: self.remote.clone(),
            });
        }
        // Checked before anything is fetched: landing more commits on top of a
        // tree the user has not finished untangling makes it strictly harder.
        if self.is_conflicted()? {
            return Err(GitError::Conflicted);
        }

        checked(run(&self.root, ["fetch", &self.remote])?, "fetch")?;
        // Two counts, deliberately. The merge has to happen whenever the remote
        // is ahead at all, or this copy silently falls behind; the number the
        // user is told has to be about their data folder, or an unrelated
        // source commit in the same repository reads as a colleague's report.
        let behind_by = self.incoming_commit_count()?;
        let received = self.incoming_data_commit_count()?;

        // `--ff-only` is the refusal: it advances the branch or it fails, and
        // it never writes a merge nobody reviewed.
        if behind_by > 0 {
            let pull = run(&self.root, ["merge", "--ff-only", "FETCH_HEAD"])?;
            if !pull.status.success() {
                return Err(GitError::Diverged {
                    remote: self.remote.clone(),
                });
            }
        }

        let changes = self.local_changes()?;
        if changes.is_empty() {
            return Ok(SyncOutcome {
                received,
                shared: Vec::new(),
            });
        }

        checked(
            run(&self.root, ["add", "--all", "--", &self.pathspec()])?,
            "add",
        )?;
        // `--only` restricts the commit to these paths, so anything the user
        // happens to have staged elsewhere in a larger repository stays staged
        // and uncommitted rather than riding along.
        checked(
            run(
                &self.root,
                [
                    "commit",
                    "--only",
                    "--message",
                    message,
                    "--",
                    &self.pathspec(),
                ],
            )?,
            "commit",
        )?;

        let push = run(&self.root, ["push", &self.remote])?;
        if !push.status.success() {
            let message = combined_output(&push);
            // The remote moved between the fetch and the push. The commit is
            // made and safe; syncing again will fast-forward and push it.
            if message.contains("rejected") || message.contains("non-fast-forward") {
                return Err(GitError::PushRejected {
                    remote: self.remote.clone(),
                });
            }
            return Err(GitError::CommandFailed {
                command: "push".to_string(),
                message,
            });
        }

        Ok(SyncOutcome {
            received,
            shared: changes.into_iter().map(|change| change.path).collect(),
        })
    }

    /// How many changes are waiting on the remote for this data folder.
    ///
    /// Fetches and counts, and does nothing else: no commit, no push, no merge,
    /// and not a single byte written to the working tree. That is the whole
    /// point — a person needs to be able to ask whether a colleague has filed
    /// something without that question also shipping their own half-finished
    /// work off the machine.
    ///
    /// The count is confined to the data folder by pathspec, so commits that
    /// happen to share the repository are not reported as colleagues' changes.
    /// Diverged histories still count: the remote's commits are genuinely
    /// there, waiting, even though [`sync`](Self::sync) will refuse them until
    /// the divergence is resolved.
    ///
    /// This talks to the network, so it belongs off the frame thread — see
    /// [`BackgroundCheck`].
    pub fn incoming(&self) -> Result<usize, GitError> {
        if !self.has_remote()? {
            return Err(GitError::NoRemote {
                remote: self.remote.clone(),
            });
        }
        checked(run(&self.root, ["fetch", &self.remote])?, "fetch")?;
        self.incoming_data_commit_count()
    }

    /// How many commits the fetched remote branch is ahead by, anywhere in the
    /// repository.
    ///
    /// This is the fast-forward decision, not the number anyone is shown: the
    /// branch has to advance whenever the remote is ahead, whatever the commits
    /// happen to touch.
    fn incoming_commit_count(&self) -> Result<usize, GitError> {
        self.count_revisions(&[])
    }

    /// How many of those commits touch the data folder.
    ///
    /// This is the number a person is shown. See the crate documentation for
    /// why the two differ.
    fn incoming_data_commit_count(&self) -> Result<usize, GitError> {
        self.count_revisions(&["--", &self.pathspec()])
    }

    /// Counts the commits in `HEAD..FETCH_HEAD`, optionally narrowed by a
    /// pathspec.
    fn count_revisions(&self, narrowing: &[&str]) -> Result<usize, GitError> {
        let mut arguments = vec!["rev-list", "--count", "HEAD..FETCH_HEAD"];
        arguments.extend_from_slice(narrowing);

        let output = run(&self.root, arguments)?;
        if !output.status.success() {
            // No FETCH_HEAD yet, or an empty repository. Nothing incoming is
            // the truthful answer, and the push below will report anything
            // genuinely wrong.
            return Ok(0);
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0))
    }
}

/// Whether a porcelain status code marks an unresolved conflict.
///
/// Git spells a conflict as `DD`, `AU`, `UD`, `UA`, `DU`, `AA` or `UU` — every
/// combination where either side is `U`, plus the two same-letter pairs.
fn is_conflict_status(status: &str) -> bool {
    let bytes = status.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    let (left, right) = (bytes[0], bytes[1]);
    left == b'U' || right == b'U' || (left == right && (left == b'A' || left == b'D'))
}

/// Runs a git command inside `directory`.
fn run<I, S>(directory: &Path, arguments: I) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let result = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        // See the crate docs: a graphical application has no terminal to answer
        // a credential prompt, so git must fail rather than wait forever.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output();

    match result {
        Ok(output) => Ok(output),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(GitError::GitNotAvailable)
        }
        Err(error) => Err(GitError::Io(error)),
    }
}

/// Turns a non-zero exit into a [`GitError::CommandFailed`].
fn checked(output: Output, command: &str) -> Result<Output, GitError> {
    if output.status.success() {
        return Ok(output);
    }
    Err(GitError::CommandFailed {
        command: command.to_string(),
        message: combined_output(&output),
    })
}

/// What git said, preferring stderr and falling back to stdout.
fn combined_output(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A sync running on a worker thread.
///
/// Talking to a remote takes as long as the network takes, and a graphical
/// application that does it on the frame thread simply stops repainting — the
/// window greys out and the user cannot even read the status line explaining
/// why. Both frontends need exactly this, so it lives here rather than being
/// written twice.
///
/// The thread is detached: nothing needs to join it, and dropping this while a
/// sync is in flight lets it finish and discard the result. Git is doing the
/// work either way, and an abandoned push is better finished than half done.
pub struct BackgroundSync {
    receiver: std::sync::mpsc::Receiver<Result<SyncOutcome, GitError>>,
}

impl std::fmt::Debug for BackgroundSync {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BackgroundSync { .. }")
    }
}

impl BackgroundSync {
    /// Starts a sync and returns immediately.
    pub fn start(repository: Repository, message: impl Into<String>) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        let message = message.into();
        std::thread::spawn(move || {
            // A closed channel means the caller has gone; the sync still ran,
            // which is the point, so the failure to report is not an error.
            let _ = sender.send(repository.sync(&message));
        });
        Self { receiver }
    }

    /// The result, once there is one. Never blocks.
    ///
    /// Returns `None` while the sync is still running *and* after its result
    /// has already been taken, so a caller polling every frame gets the outcome
    /// exactly once.
    pub fn poll(&self) -> Option<Result<SyncOutcome, GitError>> {
        self.receiver.try_recv().ok()
    }
}

/// A check for incoming changes running on a worker thread.
///
/// The same shape as [`BackgroundSync`] and for the same reason — a fetch takes
/// as long as the network takes — but for [`Repository::incoming`], which only
/// looks. Because it only looks, a frontend can start one on a timer without
/// ever pushing something the user had not decided to share.
///
/// Both this and a [`BackgroundSync`] fetch into the same `FETCH_HEAD`, so a
/// caller should not run one while the other is in flight; the frontends
/// enforce that by disabling each while the other runs.
pub struct BackgroundCheck {
    receiver: std::sync::mpsc::Receiver<Result<usize, GitError>>,
}

impl std::fmt::Debug for BackgroundCheck {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BackgroundCheck { .. }")
    }
}

impl BackgroundCheck {
    /// Starts a check and returns immediately.
    pub fn start(repository: Repository) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(repository.incoming());
        });
        Self { receiver }
    }

    /// How many changes are waiting, once the check has finished. Never blocks.
    ///
    /// Returns `None` while the check is still running *and* after its result
    /// has already been taken, exactly like [`BackgroundSync::poll`].
    pub fn poll(&self) -> Option<Result<usize, GitError>> {
        self.receiver.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether git is available to test against.
    ///
    /// The suite must build and test on a machine without git, so every test
    /// that needs it skips instead of failing. A skipped test is visible in the
    /// output; a test that silently passes on a broken assumption is not.
    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    /// A repository with one commit and an `origin` it can actually push to.
    fn repository_with_origin() -> (tempfile::TempDir, Repository) {
        let temporary = tempfile::tempdir().unwrap();
        let remote = temporary.path().join("remote.git");
        let working = temporary.path().join("working");
        std::fs::create_dir_all(&working).unwrap();

        run_ok(
            temporary.path(),
            &[
                "init",
                "--bare",
                "--initial-branch=main",
                remote.to_str().unwrap(),
            ],
        );
        run_ok(
            temporary.path(),
            &["init", "--initial-branch=main", working.to_str().unwrap()],
        );
        configure(&working);
        run_ok(
            &working,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );

        std::fs::write(working.join("first.txt"), "one\n").unwrap();
        run_ok(&working, &["add", "--all"]);
        run_ok(&working, &["commit", "--message", "first"]);
        run_ok(&working, &["push", "--set-upstream", "origin", "main"]);

        let repository = Repository::containing(&working).unwrap().unwrap();
        (temporary, repository)
    }

    /// An identity, so committing works on a machine with no global git config.
    fn configure(directory: &Path) {
        run_ok(directory, &["config", "user.email", "test@example.invalid"]);
        run_ok(directory, &["config", "user.name", "Test"]);
        run_ok(directory, &["config", "commit.gpgsign", "false"]);
    }

    fn run_ok(directory: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn a_folder_outside_a_repository_is_not_an_error() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(Repository::containing(temporary.path()).unwrap(), None);
    }

    #[test]
    fn a_missing_folder_is_not_an_error() {
        assert_eq!(
            Repository::containing("/nonexistent/for/this/test").unwrap(),
            None
        );
    }

    #[test]
    fn a_repository_is_found_from_a_folder_inside_it() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (temporary, repository) = repository_with_origin();
        let nested = repository.root().join("teams");
        std::fs::create_dir_all(&nested).unwrap();

        let found = Repository::containing(&nested).unwrap().unwrap();
        // Compared canonically: macOS puts temporary folders behind /private.
        assert_eq!(
            found.root().canonicalize().unwrap(),
            repository.root().canonicalize().unwrap()
        );
        drop(temporary);
    }

    #[test]
    fn local_changes_include_new_files() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (_temporary, repository) = repository_with_origin();
        std::fs::write(repository.root().join("second.txt"), "two\n").unwrap();

        let changes = repository.local_changes().unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, PathBuf::from("second.txt"));
        assert!(changes[0].is_new());
    }

    #[test]
    fn syncing_with_nothing_to_do_is_quiet() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (_temporary, repository) = repository_with_origin();

        let outcome = repository.sync("nothing").unwrap();
        assert!(outcome.is_quiet());
        assert_eq!(outcome.summary(), "Already up to date with everybody else.");
    }

    #[test]
    fn syncing_shares_a_new_file() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (_temporary, repository) = repository_with_origin();
        std::fs::write(repository.root().join("roster.syon"), "company: x\n").unwrap();

        let outcome = repository.sync("Add a roster").unwrap();
        assert_eq!(outcome.shared, vec![PathBuf::from("roster.syon")]);
        // And the working tree is clean afterwards.
        assert!(repository.local_changes().unwrap().is_empty());
    }

    #[test]
    fn a_colleagues_change_is_received() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (temporary, repository) = repository_with_origin();

        // A second clone stands in for the colleague.
        let colleague = temporary.path().join("colleague");
        run_ok(
            temporary.path(),
            &[
                "clone",
                temporary.path().join("remote.git").to_str().unwrap(),
                colleague.to_str().unwrap(),
            ],
        );
        configure(&colleague);
        std::fs::write(colleague.join("theirs.txt"), "theirs\n").unwrap();
        run_ok(&colleague, &["add", "--all"]);
        run_ok(&colleague, &["commit", "--message", "theirs"]);
        run_ok(&colleague, &["push"]);

        let outcome = repository.sync("mine").unwrap();
        assert_eq!(outcome.received, 1);
        assert!(repository.root().join("theirs.txt").exists());
    }

    /// The whole point of the refusal: a genuine divergence stops, and the
    /// user's own work is still sitting there untouched afterwards.
    #[test]
    fn a_divergence_is_refused_and_leaves_local_work_alone() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (temporary, repository) = repository_with_origin();

        // The colleague changes a file and pushes.
        let colleague = temporary.path().join("colleague");
        run_ok(
            temporary.path(),
            &[
                "clone",
                temporary.path().join("remote.git").to_str().unwrap(),
                colleague.to_str().unwrap(),
            ],
        );
        configure(&colleague);
        std::fs::write(colleague.join("first.txt"), "theirs\n").unwrap();
        run_ok(&colleague, &["add", "--all"]);
        run_ok(&colleague, &["commit", "--message", "theirs"]);
        run_ok(&colleague, &["push"]);

        // Meanwhile this copy commits its own change to the same file, so the
        // histories genuinely diverge rather than fast-forward.
        std::fs::write(repository.root().join("first.txt"), "mine\n").unwrap();
        run_ok(repository.root(), &["add", "--all"]);
        run_ok(repository.root(), &["commit", "--message", "mine"]);

        let error = repository.sync("mine").expect_err("this must not merge");
        assert!(
            matches!(error, GitError::Diverged { .. }),
            "expected a refusal, got {error}"
        );
        // Untouched: the user's version of the file is still theirs to resolve.
        assert_eq!(
            std::fs::read_to_string(repository.root().join("first.txt")).unwrap(),
            "mine\n"
        );
    }

    /// The failure this crate was changed to make impossible: a share picking
    /// up unrelated work from elsewhere in a larger repository.
    #[test]
    fn sharing_a_subfolder_leaves_the_rest_of_the_repository_alone() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (_temporary, repository) = repository_with_origin();
        let root = repository.root().to_path_buf();

        // The data folder, and somebody else's unrelated work beside it.
        let data = root.join("teams");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("roster.syon"), "company: x\n").unwrap();
        std::fs::write(root.join("unrelated.rs"), "fn main() {}\n").unwrap();

        let scoped = Repository::containing(&data).unwrap().unwrap();
        assert_eq!(scoped.scope(), Path::new("teams"));

        // Only the roster is even reported as a change.
        let changes = scoped.local_changes().unwrap();
        assert_eq!(changes.len(), 1, "got {changes:?}");
        assert!(changes[0].path.starts_with("teams"));

        let outcome = scoped.sync("Update the team roster").unwrap();
        assert_eq!(outcome.shared.len(), 1);

        // The unrelated file is still sitting there, uncommitted and untouched.
        let remaining = Repository::containing(&root).unwrap().unwrap();
        let left = remaining.local_changes().unwrap();
        assert!(
            left.iter()
                .any(|change| change.path == Path::new("unrelated.rs")),
            "unrelated work must not have been committed, got {left:?}"
        );
    }

    #[test]
    fn a_repository_without_a_remote_says_so() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let working = temporary.path().join("solo");
        std::fs::create_dir_all(&working).unwrap();
        run_ok(
            temporary.path(),
            &["init", "--initial-branch=main", working.to_str().unwrap()],
        );

        let repository = Repository::containing(&working).unwrap().unwrap();
        assert!(!repository.has_remote().unwrap());
        assert!(matches!(
            repository.sync("anything"),
            Err(GitError::NoRemote { .. })
        ));
    }

    #[test]
    fn conflict_statuses_are_recognised() {
        for status in ["UU", "AA", "DD", "AU", "UA", "DU", "UD"] {
            assert!(is_conflict_status(status), "{status} is a conflict");
        }
        for status in ["??", " M", "M ", "A ", "D ", "R ", ""] {
            assert!(!is_conflict_status(status), "{status} is not a conflict");
        }
    }

    #[test]
    fn a_background_sync_reports_its_outcome_exactly_once() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (_temporary, repository) = repository_with_origin();
        std::fs::write(repository.root().join("shared.syon"), "x: 1\n").unwrap();

        let running = BackgroundSync::start(repository.clone(), "Share");

        // Poll the way a frame loop would, rather than blocking.
        let mut outcome = None;
        for _ in 0..600 {
            if let Some(result) = running.poll() {
                outcome = Some(result);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let outcome = outcome
            .expect("the sync should finish")
            .expect("and succeed");
        assert_eq!(outcome.shared, vec![PathBuf::from("shared.syon")]);
        // Taken once: a frame loop must not act on the same result twice.
        assert!(running.poll().is_none());
    }

    /// Pushes a file from a second clone, so the remote is genuinely ahead.
    ///
    /// `path` is relative to the repository root, so a test can place the
    /// change inside or outside a data folder and see the difference.
    fn a_colleague_pushes(temporary: &Path, path: &str) {
        let colleague = temporary.join("colleague");
        if !colleague.exists() {
            run_ok(
                temporary,
                &[
                    "clone",
                    temporary.join("remote.git").to_str().unwrap(),
                    colleague.to_str().unwrap(),
                ],
            );
            configure(&colleague);
        }

        let file = colleague.join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&file, "theirs\n").unwrap();
        run_ok(&colleague, &["add", "--all"]);
        run_ok(&colleague, &["commit", "--message", path]);
        run_ok(&colleague, &["push"]);
    }

    /// The point of the whole feature: asking looks, and only looks.
    #[test]
    fn checking_for_incoming_changes_shares_nothing() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (temporary, repository) = repository_with_origin();
        a_colleague_pushes(temporary.path(), "theirs.txt");

        // Local work the user has not decided to share yet.
        std::fs::write(repository.root().join("mine.txt"), "mine\n").unwrap();

        assert_eq!(repository.incoming().unwrap(), 1);

        // Still uncommitted, still local, and the colleague's file has *not*
        // been merged in: a check does not touch the working tree.
        let changes = repository.local_changes().unwrap();
        assert_eq!(changes.len(), 1, "got {changes:?}");
        assert_eq!(changes[0].path, PathBuf::from("mine.txt"));
        assert!(!repository.root().join("theirs.txt").exists());

        // And asking twice gives the same answer, because nothing moved.
        assert_eq!(repository.incoming().unwrap(), 1);
    }

    /// The counting bug this replaced: with a data folder inside a larger
    /// repository, unrelated commits used to read as colleagues' changes.
    #[test]
    fn commits_outside_the_data_folder_are_not_counted() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (temporary, repository) = repository_with_origin();
        let data = repository.root().join("reports");
        std::fs::create_dir_all(&data).unwrap();
        let scoped = Repository::containing(&data).unwrap().unwrap();

        a_colleague_pushes(temporary.path(), "unrelated.rs");
        assert_eq!(
            scoped.incoming().unwrap(),
            0,
            "a source commit is not a report"
        );

        a_colleague_pushes(temporary.path(), "reports/theirs.syon");
        assert_eq!(scoped.incoming().unwrap(), 1, "but a report is");

        // The whole repository is two commits behind either way, which is what
        // the fast-forward has to act on.
        let outcome = scoped.sync("mine").unwrap();
        assert_eq!(outcome.received, 1, "still only the report is reported");
        assert!(
            repository.root().join("unrelated.rs").exists(),
            "yet both commits were merged"
        );
    }

    #[test]
    fn a_repository_without_a_remote_cannot_be_checked() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let working = temporary.path().join("solo");
        std::fs::create_dir_all(&working).unwrap();
        run_ok(
            temporary.path(),
            &["init", "--initial-branch=main", working.to_str().unwrap()],
        );

        let repository = Repository::containing(&working).unwrap().unwrap();
        assert!(matches!(
            repository.incoming(),
            Err(GitError::NoRemote { .. })
        ));
    }

    #[test]
    fn a_background_check_reports_its_count_exactly_once() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (temporary, repository) = repository_with_origin();
        a_colleague_pushes(temporary.path(), "theirs.txt");

        let running = BackgroundCheck::start(repository.clone());

        let mut result = None;
        for _ in 0..600 {
            if let Some(polled) = running.poll() {
                result = Some(polled);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            result
                .expect("the check should finish")
                .expect("and succeed"),
            1
        );
        assert!(running.poll().is_none());
    }

    #[test]
    fn a_declared_remote_is_added_when_there_is_none() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        let working = temporary.path().join("solo");
        std::fs::create_dir_all(&working).unwrap();
        run_ok(
            temporary.path(),
            &["init", "--initial-branch=main", working.to_str().unwrap()],
        );

        let repository = Repository::containing(&working).unwrap().unwrap();
        assert_eq!(repository.remote_url().unwrap(), None);

        let outcome = repository
            .ensure_remote("git@example.invalid:beriah/reports.git")
            .unwrap();
        assert_eq!(
            outcome,
            RemoteOutcome::Added {
                url: "git@example.invalid:beriah/reports.git".to_string()
            }
        );
        assert_eq!(
            repository.remote_url().unwrap().as_deref(),
            Some("git@example.invalid:beriah/reports.git")
        );

        // Asking twice writes nothing, so a frontend can call it every start-up.
        let again = repository
            .ensure_remote("git@example.invalid:beriah/reports.git")
            .unwrap();
        assert_eq!(again, RemoteOutcome::Unchanged);
        assert!(!again.changed_anything());
    }

    /// Moving where somebody's data gets pushed must be reportable, so the
    /// previous URL comes back with the outcome.
    #[test]
    fn a_remote_pointing_elsewhere_is_retargeted_and_says_so() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (_temporary, repository) = repository_with_origin();
        let previous = repository.remote_url().unwrap().expect("origin exists");

        let outcome = repository
            .ensure_remote("git@example.invalid:beriah/reports.git")
            .unwrap();
        assert_eq!(
            outcome,
            RemoteOutcome::Retargeted {
                previous: previous.clone(),
                url: "git@example.invalid:beriah/reports.git".to_string()
            }
        );
        assert!(outcome
            .summary()
            .expect("a change is worth a sentence")
            .contains(&previous));
    }

    /// The guard that keeps a settings file from rewriting an unrelated
    /// repository: a reports folder inside a source checkout must not repoint
    /// that checkout's `origin`.
    #[test]
    fn a_subfolder_may_not_configure_the_remote_of_the_repository_around_it() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let (_temporary, repository) = repository_with_origin();
        let before = repository.remote_url().unwrap().expect("origin exists");

        let data = repository.root().join("reports");
        std::fs::create_dir_all(&data).unwrap();
        let scoped = Repository::containing(&data).unwrap().unwrap();
        assert_eq!(scoped.scope(), Path::new("reports"));

        let error = scoped
            .ensure_remote("git@example.invalid:beriah/reports.git")
            .expect_err("a subfolder does not own the remote");
        assert!(
            matches!(error, GitError::NotItsOwnRepository { .. }),
            "expected a refusal, got {error}"
        );

        // And the surrounding repository still points where it did.
        assert_eq!(repository.remote_url().unwrap(), Some(before));
    }

    /// Configuring a remote is not the same as creating a repository.
    #[test]
    fn a_folder_outside_a_repository_is_still_never_created() {
        if !git_available() {
            eprintln!("skipping: git is not installed");
            return;
        }
        let temporary = tempfile::tempdir().unwrap();
        assert_eq!(Repository::containing(temporary.path()).unwrap(), None);
        assert!(!temporary.path().join(".git").exists());
    }

    #[test]
    fn the_summary_reads_as_a_sentence() {
        let quiet = SyncOutcome {
            received: 0,
            shared: Vec::new(),
        };
        assert!(quiet.is_quiet());
        let both = SyncOutcome {
            received: 2,
            shared: vec![PathBuf::from("a"), PathBuf::from("b")],
        };
        assert_eq!(both.summary(), "Received 2 change(s) and shared 2 file(s).");
    }
}
