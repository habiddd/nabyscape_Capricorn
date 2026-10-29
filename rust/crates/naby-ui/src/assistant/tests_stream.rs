//! Streaming and interaction tests that need a response body released in stages.

use std::io::{Cursor, Read};
use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::time::Duration;

use naby_core::assistant::transport::HttpResponse;
use naby_render::snapshot::save;
use serde_json::json;

use super::tests::Rig;

/// A response body that delivers its first chunk at once and the rest when released.
struct Gated {
    head: Cursor<Vec<u8>>,
    tail: Cursor<Vec<u8>>,
    gate: Mutex<Receiver<()>>,
    released: bool,
}

impl Read for Gated {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.head.read(buf)?;
        if n > 0 {
            return Ok(n);
        }
        if !self.released {
            let _ = self
                .gate
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5));
            self.released = true;
        }
        self.tail.read(buf)
    }
}

fn chunk(text: &str) -> String {
    format!(
        "data: {}\n\n",
        json!({"choices": [{"delta": {"content": text}}]})
    )
}

#[test]
fn streaming_deltas_appear_in_the_view_while_the_reply_is_still_running() {
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(Some(gate));
    let Some(mut rig) = Rig::new(true, move |_| {
        let rest = format!(
            "{}data: {}\n\ndata: [DONE]\n\n",
            chunk("lo **world**"),
            json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})
        );
        let mut response = HttpResponse::new(200, "text/event-stream", Vec::new());
        response.body = Box::new(Gated {
            head: Cursor::new(chunk("Hel").into_bytes()),
            tail: Cursor::new(rest.into_bytes()),
            gate: Mutex::new(gate.lock().unwrap().take().unwrap()),
            released: false,
        });
        Ok(response)
    }) else {
        return;
    };
    rig.settle();
    rig.type_text("greet me");
    rig.press_enter();
    let model = rig.wait(|m| m.live.as_deref() == Some("Hel"));
    assert!(
        model.busy,
        "the turn is still running while text streams in"
    );
    assert_eq!(model.items.len(), 1, "only the user message is final");
    save("assistant-streaming", &rig.canvas);
    let streaming_ink = rig.ink();
    release.send(()).unwrap();
    let model = rig.wait(|m| !m.busy && m.items.len() == 2);
    assert_eq!(model.items[1].text, "Hello **world**");
    assert!(model.live.is_none());
    assert!(rig.ink() > 0 && streaming_ink > 0);
}
