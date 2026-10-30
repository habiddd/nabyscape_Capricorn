//! Media logic ported from `Features/Media/*`: snapshots, LRC lyrics, closed-dock
//! lyric phrasing, spectrum DSP, cover palette, mixer models and policies.
//!
//! Nothing here touches the OS. The Windows host fills the snapshot structs and
//! feeds samples; views read the results.

pub mod ambient;
pub mod closed_lyrics;
pub mod lrc;
pub mod lyrics;
pub mod mixer;
pub mod palette;
pub mod peak_cache;
pub mod policy;
pub mod snapshot;
pub mod source;
pub mod spectrum;

pub use ambient::{AmbientColors, AmbientConfig, AmbientEnvelope, AmbientResponse, AmbientScope};
pub use closed_lyrics::{ClosedLine, ClosedLyrics, ClosedLyricsConfig, LyricsAnimation};
pub use lrc::{LyricLine, Lyrics};
pub use lyrics::{FetchError, LyricResult, LyricsRequest, LyricsService};
pub use mixer::{
    adjust, bounded_volume, MixerError, MixerItem, MixerResult, MixerScope, MixerSnapshot,
    MixerTarget,
};
pub use palette::CoverPalette;
pub use peak_cache::PeakCache;
pub use snapshot::{AudioOutput, AudioSnapshot, MediaSnapshot};
pub use source::MediaSourceSelection;
pub use spectrum::{Spectrum, BANDS};
