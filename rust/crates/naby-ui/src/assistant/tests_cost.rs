//! Frame cost and memory measurements (`cargo test -p naby-ui -- --ignored --nocapture cost`).

use std::time::Instant;

use naby_render::InputState;

use super::tests::{chat_reply, red_png, Rig};
use naby_core::assistant::attachments::data_url;

#[test]
#[ignore = "measurement, not a check"]
fn cost_of_an_idle_chat_frame() {
    let image = data_url("image/png", &red_png(640, 360));
    let reply = format!(
        "## Heading\n\nSome **bold** text and `code`.\n\n- one\n- two\n\n```rust\nfn main() {{}}\n```\n\n![x]({image})"
    );
    let Some(mut rig) = Rig::new(true, move |_| Ok(chat_reply(&reply))) else {
        return;
    };
    rig.settle();
    for _ in 0..3 {
        rig.view.set_composer("hi");
        rig.press_enter();
        rig.wait(|m| !m.busy);
    }
    rig.settle();
    let start = Instant::now();
    let frames = 60;
    for _ in 0..frames {
        rig.frame(InputState::default());
    }
    let per_frame = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
    println!("idle chat frame at 720x700: {per_frame:.2} ms");
    println!("canvas bytes: {}", rig.canvas.memory_bytes());
}
