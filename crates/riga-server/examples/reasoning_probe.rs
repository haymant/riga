//! Diagnostic: load a local GGUF, run one short turn, and print the raw model
//! output plus the resolved reasoning dialect. Used to see whether a model emits
//! its reasoning tags inline or relies on the template opening the block.
//!
//!   cargo run -p riga-server --features cuda --example reasoning_probe -- <model.gguf> [prompt]

use riga_server::local_model::{ChatMessage, LocalModelRuntime};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: reasoning_probe <model.gguf> [prompt]");
    let prompt = args
        .next()
        .unwrap_or_else(|| "List three prime numbers and explain briefly.".to_string());
    // The app injects a tool block into the system prompt; passing one here
    // checks whether that suppresses the model's inline `thinking` tag.
    let system = args
        .next()
        .unwrap_or_else(|| "You are a helpful assistant.".to_string());

    let runtime = LocalModelRuntime::default();
    runtime.load_model(&path).await.expect("load model");
    println!("dialect: {:?}", runtime.reasoning_format());

    let messages = vec![
        ChatMessage {
            role: "system".into(),
            content: system,
        },
        ChatMessage {
            role: "user".into(),
            content: prompt,
        },
    ];
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let expected = std::sync::atomic::AtomicBool::new(false);
    let mut raw = String::new();
    let generated = runtime
        .generate(
            &messages,
            256,
            None,
            None,
            64,
            &cancel,
            &expected,
            |delta| raw.push_str(delta),
        )
        .expect("generate");

    println!(
        "opens_in_prompt: {}",
        expected.load(std::sync::atomic::Ordering::Relaxed)
    );
    println!("tokens: {}", generated.tokens);
    println!("----- RAW OUTPUT (first 600 chars) -----");
    println!("{}", raw.chars().take(600).collect::<String>());
    println!("----- STRIPPED (final answer) -----");
    println!("{}", runtime.reasoning_format().strip(&raw));
}
