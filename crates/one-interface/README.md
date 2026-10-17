# one-interface

The window every egui frontend in the Beriah suite opens: its icon, and its
text at the size the user asked for.

The suite is heading for more than ten desktop applications. Each needs the same
two things before it can draw anything, and both were being written out per
application — `apply_font_size` was byte-identical in three `main.rs` files, and
the icon was on its way to a fourth and fifth copy. This crate is where they
live once.

## What an application writes now

```rust
let native_options = eframe::NativeOptions {
    viewport: one_interface::viewport([1000.0, 720.0], one_interface::application_icon!()),
    ..Default::default()
};

eframe::run_native("Beriah Reporter", native_options, Box::new(move |creation_context| {
    one_interface::apply_font_size(&creation_context.egui_ctx, &settings.interface);
    Ok(Box::new(ReporterApplication::configured(&settings.storage)))
}))
```

Two lines, replacing about thirty per application.

## Why the icon is a macro

`include_bytes!` resolves its path against the source file it appears in, at
compile time, so a library **function** cannot embed a file on a caller's
behalf. A macro can: it expands in the calling crate, where `CARGO_MANIFEST_DIR`
and `CARGO_PKG_NAME` are that crate's own. So `application_icon!` carries the
whole thing — the path convention, the embed, the decode, and the application
name that appears in any error message.

With no argument it takes `../../assets/app_icon__256px.png`, since every
application sits at `crates/<project>/crates/app` and its workspace's `assets/`
is two levels up. An application shaped differently passes its own path literal.

The path must be a literal rather than a named constant, because `concat!`
cannot take one.

## Why not `one-settings`

`one-settings` is a member of the repository-root workspace, so anything it
depends on gets compiled to build the SYON parser. It must stay free of
graphical-toolkit dependencies for that reason. This crate is its toolkit-aware
sibling and is **excluded** from the root workspace, which is what keeps
`cargo build` at the repository root from pulling in eframe, winit and glow.

The division between them is worth keeping:

| Crate | Owns |
|---|---|
| `one-settings` | What the number means — `scale_factor_from`, range checking. Testable with no window. |
| `one-interface` | The handful of lines that reach into `egui::Style`, and the window itself. |

## Icons: PNG ships, SVG is edited

`egui::IconData` is raw RGBA, and `eframe::icon_data::from_png_bytes` is the
only decoder eframe brings along — so PNG is what an application can embed. Each
application keeps `assets/app_icon.svg` as the source and renders it with
`task icons`.

A decode failure yields `None`, never a placeholder. A window with no icon gets
the system default; a window handed a transparent one-pixel stand-in gets a
blank square, which reads as a broken application.

The icon shows on **Windows and X11**. winit documents `set_window_icon` as
unsupported on macOS and Wayland, where it comes from the application bundle
instead — a developer on a Mac seeing no icon is looking at expected behaviour.
