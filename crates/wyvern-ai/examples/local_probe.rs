//! Asks the local model (Ollama) for two suggestions and prints them with their timing.
//!
//! `cargo run -p wyvern-ai --example local_probe [model]`

use std::sync::mpsc;
use std::time::Instant;

use wyvern_ai::{clean_suggestion, request, Backend, Context, Event, Generation, DEFAULT_LOCAL_MODEL};

fn main() {
    let model = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_LOCAL_MODEL.to_string());
    let cases = [
        (
            "using System;
using System.Collections.Generic;

class Program
{
    static void Main()
    {
        var numbers = new List<int> { 4, 8, 15, 16, 23, 42 };
        int count = 0;
        for(",
            ")
    }
}
",
        ),
        (
            "using System;\n\nclass Program\n{\n    static int Sum(int[] values)\n    {\n        int total = 0;\n        foreach (var v in values)\n        {\n            ",
            "\n        }\n        return total;\n    }\n}\n",
        ),
        (
            "class Player\n{\n    public string Name { get; set; }\n    public int Health { get; set; }\n\n    public bool IsDead()\n    {\n        ",
            "\n    }\n}\n",
        ),
    ];
    let generation = Generation::default();
    for (i, (before, after)) in cases.iter().enumerate() {
        // The first run loads the model into the GPU; run each case twice to see both times.
        for attempt in 0..2 {
            let (tx, rx) = mpsc::channel();
            let id = generation.next();
            let start = Instant::now();
            let ctx = Context {
                file_name: "Program.cs".into(),
                language: "C#".into(),
                before: before.to_string(),
                after: after.to_string(),
            };
            request(Backend::Local(model.clone()), None, ctx, generation.clone(), id, move |_, ev| {
                let _ = tx.send(ev);
            });
            let mut first = None;
            loop {
                match rx.recv().expect("request thread ended") {
                    Event::Partial(_) => {
                        first.get_or_insert(start.elapsed());
                    }
                    Event::Done(text) => {
                        let shown = clean_suggestion(&text, after);
                        println!(
                            "case {i} run {attempt}: first text after {:?}, done after {:?}\n---\n{shown}\n---",
                            first.unwrap_or_default(),
                            start.elapsed()
                        );
                        break;
                    }
                    Event::Failed(f) => {
                        println!("case {i} run {attempt}: failed: {f:?}");
                        break;
                    }
                }
            }
        }
    }
}
