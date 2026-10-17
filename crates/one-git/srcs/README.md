# one-git — sources

| File | Contents |
|---|---|
| `lib.rs` | The whole crate: `Repository` discovery and `sync`, `BackgroundSync`, `GitError`, and the tests that drive real repositories. |

Single-file because the surface is small and every part of it is about one
subprocess call. See the crate's [README](../README.md) for the decisions behind
it — shelling out to `git`, refusing rather than merging, and never creating a
repository.
