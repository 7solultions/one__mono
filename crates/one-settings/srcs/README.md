# one-settings — sources

| File | Contents |
|---|---|
| `lib.rs` | `settings.syon`: the `interface` and `features` sections, path resolution, and parsing. |
| `info.rs` | `info.syon`: who the person at this machine is, as a company id and a participant slot — never a name. |

The split is the point: preferences are shared and portable, identity is not, so
a settings file can be copied between machines without carrying an identity
along with it.
