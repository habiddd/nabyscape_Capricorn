//! CPU vector painter (tiny-skia), font rasterizer (fontdue) and immediate-mode widgets.
//!
//! Buffers are sized to what is on screen and dropped when a surface hides, which keeps
//! resident memory to a few megabytes instead of a GPU swap chain plus retained visual tree.
