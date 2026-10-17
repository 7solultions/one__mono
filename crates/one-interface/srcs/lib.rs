//! The window every egui frontend in the Beriah suite opens.
//!
//! The suite is heading for more than ten desktop applications, and each one
//! needs the same two things before it can draw anything: an icon, and text at
//! the size the user asked for. Both were being written out per application —
//! [`apply_font_size`] was byte-identical in three `main.rs` files, and the icon
//! was on its way to being copied a fourth and fifth time. This crate is where
//! they live once.
//!
//! ## Why this is not `one-settings`
//!
//! [`one_settings`] must stay free of graphical-toolkit dependencies: it is a
//! member of the repository-root workspace, so anything it depends on is
//! compiled to build the SYON parser. This crate is its toolkit-aware sibling
//! and is excluded from that workspace for exactly that reason.
//!
//! The division of labour between them is worth keeping: `one-settings` owns
//! *what the number means* — [`InterfaceSettings::scale_factor_from`] and its
//! range checking, testable without a window — and this crate owns the handful
//! of lines that reach into `egui::Style`.
//!
//! ## Why the icon is a macro
//!
//! `include_bytes!` resolves its path against the source file it appears in, at
//! compile time, so a library function cannot embed a file on a caller's
//! behalf. A macro can: it expands *in the calling crate*, where
//! `CARGO_MANIFEST_DIR` and `CARGO_PKG_NAME` are that crate's own. So
//! [`application_icon`] carries the whole thing — the path convention, the
//! embed, the decode, and the application name that appears in the error — and
//! an application spends one line on its icon rather than thirty.
//!
//! ```ignore
//! let native_options = eframe::NativeOptions {
//!     viewport: one_interface::viewport([1000.0, 720.0], one_interface::application_icon!()),
//!     ..Default::default()
//! };
//! ```

use eframe::egui;
use egui::IconData;
use one_settings::InterfaceSettings;

/// Embeds and decodes this application's window icon.
///
/// With no argument it takes `../../assets/app_icon__256px.png`: every
/// application in the suite sits at `crates/<project>/crates/app`, so its
/// workspace's `assets/` is two levels up. Stating that convention once is most
/// of the reason this macro exists. An application shaped differently passes
/// its own path literal instead.
///
/// The path is resolved against the calling **crate's manifest directory**
/// rather than the source file, so it reads the same wherever in the crate it
/// is invoked. It has to be a literal — `concat!` cannot take a constant, which
/// is also why the default is spelled out below rather than named.
///
/// Yields `Option<IconData>`, never panicking: see [`icon_from_png_bytes`] for
/// why a missing icon beats a broken one.
///
/// ```ignore
/// one_interface::application_icon!()                       // the convention
/// one_interface::application_icon!("../assets/other.png")  // anywhere else
/// ```
#[macro_export]
macro_rules! application_icon {
    () => {
        $crate::application_icon!("../../assets/app_icon__256px.png")
    };
    ($path:literal) => {
        $crate::icon_from_png_bytes(
            env!("CARGO_PKG_NAME"),
            include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
        )
    };
}

/// Decodes PNG bytes into an icon egui can use.
///
/// Yields `None` rather than a placeholder on failure, and the difference is
/// visible: a window with no icon gets the system default, while a window given
/// a transparent one-pixel stand-in gets a blank square, which reads as a
/// broken application. `application_name` names the culprit in the message,
/// because a suite of ten windows needs to say *which* one.
///
/// PNG is not a preference. [`egui::IconData`] is raw RGBA, and
/// `eframe::icon_data::from_png_bytes` is the only decoder eframe brings along
/// — so PNG is what an application can ship, and SVG is what a person edits.
pub fn icon_from_png_bytes(application_name: &str, png_bytes: &[u8]) -> Option<IconData> {
    match eframe::icon_data::from_png_bytes(png_bytes) {
        Ok(icon) => Some(icon),
        Err(error) => {
            eprintln!("{application_name}: the embedded window icon could not be decoded: {error}");
            None
        }
    }
}

/// The viewport an application opens with.
///
/// Takes the icon rather than finding it, because finding it is the macro's job
/// — see [`application_icon`]. A `None` icon simply never calls `with_icon`, so
/// the window falls back to the system default.
///
/// The icon shows on Windows and X11. winit documents `set_window_icon` as
/// unsupported on **macOS and Wayland**, where it comes from the application
/// bundle instead — so a developer on a Mac seeing no icon is looking at
/// expected behaviour, not a bug.
pub fn viewport(inner_size: [f32; 2], icon: Option<IconData>) -> egui::ViewportBuilder {
    let mut builder = egui::ViewportBuilder::default().with_inner_size(inner_size);
    if let Some(icon) = icon {
        builder = builder.with_icon(std::sync::Arc::new(icon));
    }
    builder
}

/// Scales every text style so body text lands on the size the user configured.
///
/// Scaling rather than assigning is what keeps egui's type hierarchy intact: a
/// toolkit defines its heading, button, small and monospace sizes relative to
/// body text, so assigning the configured size to each style would flatten the
/// hierarchy into one size. The factor comes from
/// [`InterfaceSettings::scale_factor_from`], which owns the arithmetic.
///
/// Spacing gets the same factor because it is in points rather than ems, and
/// larger text would otherwise sit cramped against its own borders.
///
/// A context whose body style is missing is left untouched: there is nothing
/// meaningful to scale against, and guessing would be worse than doing nothing.
pub fn apply_font_size(context: &egui::Context, interface: &InterfaceSettings) {
    let mut style = (*context.style()).clone();

    let current_body_size = match style.text_styles.get(&egui::TextStyle::Body) {
        Some(font) => font.size,
        None => return,
    };
    let factor = interface.scale_factor_from(current_body_size);

    for (_text_style, font) in style.text_styles.iter_mut() {
        font.size *= factor;
    }
    style.spacing.item_spacing *= factor;
    style.spacing.button_padding *= factor;
    style.spacing.interact_size *= factor;

    context.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the setting: whatever the file says, body text ends
    /// up at exactly that many points.
    #[test]
    fn body_text_lands_on_the_configured_size() {
        for font_size in [12, 18, 25] {
            let context = egui::Context::default();
            apply_font_size(&context, &InterfaceSettings { font_size });

            let body = context.style().text_styles[&egui::TextStyle::Body].size;
            assert!(
                (body - font_size as f32).abs() < 0.001,
                "font_size {font_size} produced body text of {body}"
            );
        }
    }

    /// And the rest of the hierarchy moves with it rather than collapsing onto
    /// it — a heading stays a heading.
    ///
    /// Moved here from the frontends, where three copies of it were checking
    /// three copies of this function.
    #[test]
    fn every_text_style_scales_by_the_same_factor() {
        let interface = InterfaceSettings { font_size: 25 };
        let context = egui::Context::default();

        let before: Vec<f32> = context
            .style()
            .text_styles
            .values()
            .map(|font| font.size)
            .collect();
        let factor =
            interface.scale_factor_from(context.style().text_styles[&egui::TextStyle::Body].size);

        apply_font_size(&context, &interface);

        let after: Vec<f32> = context
            .style()
            .text_styles
            .values()
            .map(|font| font.size)
            .collect();

        assert_eq!(before.len(), after.len());
        for (before, after) in before.iter().zip(after.iter()) {
            assert!(
                (after - before * factor).abs() < 0.001,
                "expected {before} to scale to {}, got {after}",
                before * factor
            );
        }
    }

    /// Spacing is in points, so it has to move with the type or the larger text
    /// sits cramped against its own borders.
    #[test]
    fn spacing_grows_with_the_text() {
        let context = egui::Context::default();
        let before = context.style().spacing.item_spacing;

        apply_font_size(&context, &InterfaceSettings { font_size: 25 });

        let after = context.style().spacing.item_spacing;
        assert!(
            after.x > before.x && after.y > before.y,
            "spacing went from {before:?} to {after:?}"
        );
    }

    /// A context with no body style has nothing to scale against, and guessing
    /// would be worse than leaving the toolkit's own sizing alone.
    #[test]
    fn a_context_without_body_text_is_left_untouched() {
        let context = egui::Context::default();
        let mut style = (*context.style()).clone();
        style.text_styles.remove(&egui::TextStyle::Body);
        let heading_before = style.text_styles[&egui::TextStyle::Heading].size;
        context.set_style(style);

        apply_font_size(&context, &InterfaceSettings { font_size: 40 });

        assert_eq!(
            context.style().text_styles[&egui::TextStyle::Heading].size,
            heading_before
        );
    }

    /// A broken icon must not take the window down with it.
    #[test]
    fn undecodable_bytes_yield_no_icon_rather_than_a_panic() {
        assert!(icon_from_png_bytes("a-test", b"not a png at all").is_none());
        assert!(icon_from_png_bytes("a-test", &[]).is_none());
    }

    /// `None` must leave the builder alone rather than painting a blank square.
    #[test]
    fn a_viewport_without_an_icon_carries_none() {
        assert!(viewport([800.0, 600.0], None).icon.is_none());

        let icon = IconData {
            rgba: vec![255, 0, 0, 255],
            width: 1,
            height: 1,
        };
        assert!(viewport([800.0, 600.0], Some(icon)).icon.is_some());
    }
}
