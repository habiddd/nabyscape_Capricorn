//! Wake word listening on `Windows.Media.SpeechRecognition` with a constrained phrase grammar
//! (`AssistantWakeWord`). It is the detector when Whistle is not the listening engine or is not
//! installed; otherwise [`super::wake_whistle`] listens. It runs only while hands-free is enabled
//! (the state machine in [`super::wake_logic`] decides when) and never uploads or stores audio.
//!
//! Windows recognizes from the default capture device; a different microphone chosen in Voice
//! settings applies to conversations, not to wake listening (the status says so).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use naby_core::assistant::voice::{wake_phrase, wake_plan, wake_threshold};
use windows::core::HSTRING;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Globalization::Language;
use windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionListConstraint, SpeechRecognitionResult, SpeechRecognitionResultStatus,
    SpeechRecognizer,
};
use windows_collections::{IIterable, IVectorView};

use naby_ui::assistant::voice::Recognizer;

/// Windows speech languages the wake grammar can use.
pub fn recognizers() -> Vec<Recognizer> {
    let _com = crate::devices::ComApartment::mta();
    let Ok(list) = SpeechRecognizer::SupportedGrammarLanguages() else {
        return Vec::new();
    };
    let list: IVectorView<Language> = list;
    list.into_iter()
        .filter_map(|l| {
            Some(Recognizer {
                id: l.LanguageTag().ok()?.to_string(),
                name: l.DisplayName().ok()?.to_string(),
            })
        })
        .collect()
}

/// One recognizer and its continuous session.
struct Channel {
    recognizer: SpeechRecognizer,
    session: SpeechContinuousRecognitionSession,
}

/// A running wake listener: one recognizer per language. Dropping it releases them and the microphone.
pub struct WakeListener {
    channels: Vec<Channel>,
    live: Arc<AtomicBool>,
    shown: Vec<String>,
}

type Detected = Arc<dyn Fn() + Send + Sync>;
type Failed = Arc<dyn Fn(String) + Send + Sync>;

/// Compiles `phrases` for `tag` and starts a session. `live`/`fired` are shared by all channels so
/// that detection and failure each report once.
fn open(
    tag: &str,
    phrases: &[String],
    threshold: f64,
    live: &Arc<AtomicBool>,
    fired: &Arc<AtomicBool>,
    detected: &Detected,
    failed: &Failed,
) -> Result<Channel, String> {
    let language = Language::CreateLanguage(&HSTRING::from(tag)).map_err(|e| e.message())?;
    let recognizer = SpeechRecognizer::Create(&language).map_err(|e| e.message())?;
    let phrases: Vec<HSTRING> = phrases.iter().map(HSTRING::from).collect();
    let iterable = IIterable::<HSTRING>::from(phrases);
    let constraint =
        SpeechRecognitionListConstraint::CreateWithTag(&iterable, &HSTRING::from("naby-wake"))
            .map_err(|e| e.message())?;
    recognizer
        .Constraints()
        .and_then(|c| c.Append(&constraint))
        .map_err(|e| e.message())?;
    let compiled = recognizer
        .CompileConstraintsAsync()
        .and_then(|op| op.get())
        .map_err(|e| e.message())?;
    if compiled.Status().map_err(|e| e.message())? != SpeechRecognitionResultStatus::Success {
        return Err("Windows speech recognition could not compile the wake phrase. Check the installed speech language.".into());
    }
    let session = recognizer
        .ContinuousRecognitionSession()
        .map_err(|e| e.message())?;
    let (flag, once, detected) = (live.clone(), fired.clone(), detected.clone());
    session
        .ResultGenerated(&TypedEventHandler::<
            SpeechContinuousRecognitionSession,
            SpeechContinuousRecognitionResultGeneratedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let result: SpeechRecognitionResult = args.Result()?;
                if flag.load(Ordering::SeqCst)
                    && result.RawConfidence()? >= threshold
                    && !once.swap(true, Ordering::SeqCst)
                {
                    flag.store(false, Ordering::SeqCst);
                    detected();
                }
            }
            Ok(())
        }))
        .map_err(|e| e.message())?;
    let (flag, failed) = (live.clone(), failed.clone());
    session
        .Completed(&TypedEventHandler::<
            SpeechContinuousRecognitionSession,
            SpeechContinuousRecognitionCompletedEventArgs,
        >::new(move |_, args| {
            if flag.swap(false, Ordering::SeqCst) {
                let status = args.as_ref().and_then(|a| a.Status().ok());
                failed(if status == Some(SpeechRecognitionResultStatus::Success) {
                    "Wake listening stopped. Resume it to listen again.".into()
                } else {
                    "Windows speech recognition stopped. Check the microphone and selected speech language, then resume wake listening.".into()
                });
            }
            Ok(())
        }))
        .map_err(|e| e.message())?;
    // Never end on silence: Completed would pause wake listening in a quiet room.
    session
        .SetAutoStopSilenceTimeout(TimeSpan {
            Duration: 24 * 3600 * 10_000_000,
        })
        .map_err(|e| e.message())?;
    session
        .StartAsync()
        .and_then(|op| op.get())
        .map_err(|_| "Windows speech recognition could not start. Check microphone permissions and the speech language, then resume wake listening.".to_owned())?;
    Ok(Channel {
        recognizer,
        session,
    })
}

impl WakeListener {
    /// Starts listening. `detected` fires once when the phrase is heard with enough confidence;
    /// `failed` fires if recognition stops unexpectedly. With several languages it keeps listening
    /// in those that start and fails only if none does.
    pub fn start(
        phrase: &str,
        language_id: &str,
        confidence: f64,
        detected: impl Fn() + Send + Sync + 'static,
        failed: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<WakeListener, String> {
        let _com = crate::devices::ComApartment::mta();
        let phrase = wake_phrase(phrase).map_err(|e| e.message().to_owned())?;
        let available: Vec<String> = recognizers().into_iter().map(|r| r.id).collect();
        let plan = wake_plan(&phrase, language_id, &available);
        if plan.is_empty() {
            return Err("No matching Windows speech recognizer is installed. Install a speech language in Windows Settings \u{2192} Time & language \u{2192} Speech, then choose Refresh recognizers.".to_owned());
        }
        let live = Arc::new(AtomicBool::new(true));
        let fired = Arc::new(AtomicBool::new(false));
        let (detected, failed): (Detected, Failed) = (Arc::new(detected), Arc::new(failed));
        let threshold = wake_threshold(confidence);
        let mut listener = WakeListener {
            channels: Vec::new(),
            live: live.clone(),
            shown: Vec::new(),
        };
        let mut last_error = String::new();
        for (tag, phrases) in &plan {
            match open(tag, phrases, threshold, &live, &fired, &detected, &failed) {
                Ok(channel) => {
                    listener.channels.push(channel);
                    listener.shown.push(phrases[0].clone());
                }
                Err(e) => last_error = e,
            }
        }
        if listener.channels.is_empty() {
            return Err(last_error);
        }
        Ok(listener)
    }

    /// The phrase each active language listens for (the first of its grammar).
    pub fn shown(&self) -> &[String] {
        &self.shown
    }

    pub fn listening(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// Stops listening and releases the microphone.
    pub fn stop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
        for c in self.channels.drain(..) {
            let _ = c.session.CancelAsync().and_then(|op| op.get());
            let _ = c.recognizer.Close();
        }
    }
}

impl Drop for WakeListener {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real Windows speech: starts the default (bilingual) listener for `NABY_WAKE_SECS` seconds
    /// (default 2); it needs the English and Spanish recognizers and a microphone. Play a phrase
    /// meanwhile to see which detection fires (acoustic loop, may legitimately miss).
    #[test]
    #[ignore = "needs Windows speech languages and a microphone"]
    fn both_languages_listen_concurrently() {
        use std::sync::{atomic::AtomicUsize, Mutex};
        let secs = std::env::var("NABY_WAKE_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2);
        let hits = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let (h, f) = (hits.clone(), failures.clone());
        let listener = WakeListener::start(
            "Hey Naby",
            "",
            0.7,
            move || {
                h.fetch_add(1, Ordering::SeqCst);
            },
            move |m| f.lock().unwrap().push(m),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(secs));
        println!(
            "sessions: {} phrases: {:?} detections: {} listening: {} failures: {:?}",
            listener.channels.len(),
            listener.shown(),
            hits.load(Ordering::SeqCst),
            listener.listening(),
            failures.lock().unwrap()
        );
        assert!(failures.lock().unwrap().is_empty());
        assert!(listener.listening() || hits.load(Ordering::SeqCst) == 1);
        assert_eq!(listener.channels.len(), 2);
    }
}
