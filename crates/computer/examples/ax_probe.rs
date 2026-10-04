//! Spike 4 (design doc 96 §10): print what the accessibility tree reports
//! under the mouse and for the frontmost window. Run with
//! `cargo run -p computer --example ax_probe` from a terminal that has
//! Accessibility granted; pass `--watch` to poll every second.

use std::time::Duration;

use computer::pointer::Pt;
use computer::{ax, input};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let input = input::spawn();
    let watch = std::env::args().any(|a| a == "--watch");
    loop {
        match input.run(input::Action::Location).await {
            Ok(input::Reply::Location(x, y)) => {
                let at = Pt {
                    x: x as f32,
                    y: y as f32,
                };
                println!("granted: {}", ax::granted());
                println!("pointer: ({x}, {y})");
                println!("frontmost: {:#?}", ax::frontmost());
                println!("element: {:#?}", ax::element_at(at));
            }
            other => println!("pointer: {other:?}"),
        }
        if !watch {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
        println!("---");
    }
}
