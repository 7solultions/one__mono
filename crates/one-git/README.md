# one-git

Sharing a Beriah data folder with colleagues through a git repository.

Two applications keep their data as files people are meant to share:
`beriah-team` keeps rosters, shared with colleagues, and `beriah-support` keeps
reports, shared with colleagues *and* an outside support team. Both formats were
already designed for this — byte-identical re-emission, per-file change
tracking, second-precision timestamps, base64 attachments — so what was missing
was the plumbing, not the file format.

## It shells out to `git`

Every command runs the user's own `git` binary. The hard part of this feature is
not the porcelain, it is **authentication**: SSH agents, credential helpers,
corporate proxies, `includeIf` config, signing keys. Shelling out inherits
whatever already works on that machine, and when something fails the user can
run the same command by hand and get the same error.

Two environment variables are set on every invocation:

| Variable | Why |
|---|---|
| `GIT_TERMINAL_PROMPT=0` | A graphical application has no terminal to answer a credential prompt, so git must fail rather than wait forever. |
| `GIT_OPTIONAL_LOCKS=0` | A read-only query must not take the index lock and fight with a terminal open on the same folder. |

## It refuses rather than merges

`Repository::sync` fast-forwards with `merge --ff-only`. On a genuine divergence
it stops, reports `GitError::Diverged`, and leaves the working tree exactly as
it found it.

One of these repositories holds personal data and the other holds other
people's incident reports. An automatic merge would be this crate inventing a
resolution nobody reviewed, in files where being quietly wrong is expensive. Git
already has good tools for resolving a divergence and the user already has them
installed.

## Looking is separate from sharing

`Repository::incoming` fetches and counts, and does nothing else — no commit, no
push, no merge, and not a byte written to the working tree. That separation is
the point: a person needs to be able to ask whether a colleague has filed
something without the question also shipping their own half-finished work off
the machine, which is what makes it safe for a frontend to ask on a timer.

It is also the only honest way to count. `sync` must fast-forward on *any*
incoming commit, so its merge decision looks at the whole repository — but the
number a person is shown is scoped to the data folder by pathspec. Without that,
a mono-repo with the data inside it reports every unrelated source commit as a
colleague's change.

## The remote comes from settings

`Repository::ensure_remote` points the folder's remote at a declared URL, adding
it when there is none and retargeting it when it points elsewhere. The URL lives
in the suite's shared `settings.syon`, so a deployment configures sharing in one
file rather than talking every user through `git remote add` on their own
laptop. `RemoteOutcome` carries the previous URL when one is retargeted, because
silently moving where somebody's data gets pushed is the one outcome here worth
hiding and therefore the one that must not be.

This is the only thing in the crate that writes git *configuration* rather than
history, and it is fenced two ways:

- **Only when the data folder is the repository root.** A remote belongs to a
  whole repository, not to a pathspec. Retargeting one from inside a subfolder
  would repoint a repository that merely happens to contain the data — for a
  reports folder in a source checkout, that means rewriting the checkout's own
  `origin`. A subfolder gets `GitError::NotItsOwnRepository`.
- **It still does not clone or `init`.** A folder is either already a repository
  the user set up, or sharing is off for it — `Repository::containing` returns
  `Ok(None)`, an ordinary state rather than an error.

What remains true is the part worth remembering: whoever writes the settings
file decides where this data is pushed.

## Off the frame thread

`BackgroundSync::start` runs a sync on a worker thread and `poll` returns its
result exactly once, without blocking. Talking to a remote takes as long as the
network takes, and a frontend doing that on the frame thread simply stops
repainting — the window greys out and the user cannot even read the status line
explaining why.

`BackgroundCheck::start` is the same shape for `incoming`. Both fetch into the
same `FETCH_HEAD`, so a caller should not run one while the other is in flight;
the frontends disable each while the other runs.

## Tests

Every test that needs git builds real repositories in a temporary folder,
including a second clone standing in for a colleague. On a machine without git
installed they print `skipping` and pass, so the suite still builds where git
does not exist.
