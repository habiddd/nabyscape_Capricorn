//! CPU vector painter (tiny-skia), font rasterizer (fontdue) and immediate-mode widgets.
//!
//! Buffers are sized to what is on screen and dropped when a surface hides, which keeps
//! resident memory to a few megabytes instead of a GPU swap chain plus retained visual tree.
//!
//! * [`canvas`]: [`Canvas`], [`Rect`], [`Fill`] and Win32 output ([`Canvas::write_bgra_premultiplied`]).
//! * [`text`]: [`Fonts`] (lazy system fonts, 4 MiB glyph cache), measuring, wrapping, ellipsis.
//! * [`icons`]: every Lucide / filled icon of the WPF build and the SVG path parser.
//! * [`image`]: decode PNG/JPEG straight to display size, byte-bounded [`ThumbnailCache`].
//! * [`theme`]: colors, radii and sizes from `Theme.xaml`.
//! * [`ui`]: the immediate-mode widget toolkit with keyboard focus and Tab order.
//! * [`snapshot`]: helpers to write test PNGs under `target/snapshots/`.

pub mod canvas;
pub mod contour;
pub mod icons;
pub mod image;
pub mod snapshot;
pub mod text;
pub mod theme;
pub mod ui;

pub use canvas::{Canvas, Fill, Rect};
pub use icons::Icon;
pub use image::{Image, ThumbnailCache};
pub use naby_core::color::Rgba;
pub use text::{Fonts, TextStyle, Weight};
pub use theme::Theme;
pub use ui::{InputState, Key, KeyEvent, Response, TextEditOpts, Ui, UiMemory};
