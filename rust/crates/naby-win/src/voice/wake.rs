//! Wake word listening on `Windows.Media.SpeechRecognition` with a constrained phrase grammar
//! (`AssistantWakeWord`). It runs only while hands-free is enabled (the state machine in
//! [`super::wake_logic`] decides when), never uploads or stores audio, and is a different engine
//! from Whistle so that no always-on Whistle loop exists.
//!
//! Windows recognizes from the default capture device; a different microphone chosen in Voice
//! settings applies to conversations, not to wake listening (the status says so).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use naby_core::assistant::voice::{wake_grammar, wake_phrase, wake_threshold};
use windows::core::HSTRING;
use windows::Foundation::TypedEventHandler;
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

/// Picks the saved language, else `en-US`, else the first installed one.
pub fn choose_language(saved_id: &str, available: &[Recognizer]) -> Option<String> {
    if !saved_id.is_empty() {
        return available
            .iter()
            .find(|r| r.id == saved_id)
            .map(|r| r.id.clone());
    }
    available
        .iter()
        .find(|r| r.id.eq_ignore_ascii_case("en-US"))
        .or_else(|| available.first())
        .map(|r| r.id.clone())
}

/// A running wake listener. Dropping it releases the recognizer and the microphone.
pub struct WakeListener {
    recognizer: Option<SpeechRecognizer>,
    session: Option<SpeechContinuousRecognitionSession>,
    live: Arc<AtomicBool>,
}

impl WakeListener {
    /// Starts listening. `detected` fires once when the phrase is heard with enough confidence;
    /// `failed` fires if recognition stops unexpectedly.
    pub fn start(
        phrase: &str,
        language_id: &str,
        confidence: f64,
        detected: impl Fn() + Send + Sync + 'static,
        failed: impl Fn(String) + Send + Sync + 'static,
    ) -> Result<WakeListener, String> {
        let _com = crate::devices::ComApartment::mta();
        let phrase = wake_phrase(phrase).map_err(|e| e.message().to_owned())?;
        let available = recognizers();
        let tag = choose_language(language_id, &available).ok_or_else(|| {
            "No matching Windows speech recognizer is installed. Install a speech language in Windows Settings \u{2192} Time & language \u{2192} Speech, then choose Refresh recognizers.".to_owned()
        })?;
        let language = Language::CreateLanguage(&HSTRING::from(tag)).map_err(|e| e.message())?;
        let recognizer = SpeechRecognizer::Create(&language).map_err(|e| e.message())?;
        let phrases: Vec<HSTRING> = wake_grammar(&phrase)
            .into_iter()
            .map(HSTRING::from)
            .collect();
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
        let live = Arc::new(AtomicBool::new(true));
        let threshold = wake_threshold(confidence);
        let fired = Arc::new(AtomicBool::new(false));
        let (flag, once) = (live.clone(), fired.clone());
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
        let flag = live.clone();
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
        session
            .StartAsync()
            .and_then(|op| op.get())
            .map_err(|_| "Windows speech recognition could not start. Check microphone permissions and the speech language, then resume wake listening.".to_owned())?;
        Ok(WakeListener {
            recognizer: Some(recognizer),
            session: Some(session),
            live,
        })
    }

    pub fn listening(&self) -> bool {
        self.live.load(Ordering::SeqCst)
    }

    /// Stops listening and releases the microphone.
    pub fn stop(&mut self) {
        self.live.store(false, Ordering::SeqCst);
        if let Some(session) = self.session.take() {
            let _ = session.CancelAsync().and_then(|op| op.get());
        }
        if let Some(recognizer) = self.recognizer.take() {
            let _ = recognizer.Close();
        }
    }
}

impl Drop for WakeListener {
    fn drop(&mut self) {
        self.stop();
    }
}
