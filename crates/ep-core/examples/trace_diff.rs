//! Compare the simulation with an IRIX `eptrace` recording and print the
//! first divergence with context.
//!
//! ```text
//! cargo run -p electropaint-core --example trace_diff -- <trace> [--script FILE] [--quad] [--saver]
//!     [--frames N] [--seed N] [--ops eptrace_ops.json] [--context N]
//! ```
//!
//! The opcode table defaults to `eptrace_ops.json` next to the trace.

use std::path::PathBuf;
use std::process::ExitCode;

use electropaint_core::trace::{self, Tolerance};
use electropaint_core::{Options, Script};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut trace_path = None;
    let mut script = None;
    let (mut quad, mut saver) = (false, false);
    let mut frames = usize::MAX;
    let mut seed = 12345;
    let mut ops = None;
    let mut context = 8;
    while let Some(a) = args.next() {
        let mut val = |name: &str| {
            args.next()
                .unwrap_or_else(|| panic!("{name} needs a value"))
        };
        match a.as_str() {
            "--script" => script = Some(PathBuf::from(val("--script"))),
            "--quad" | "-q" => quad = true,
            "--saver" | "-S" => saver = true,
            "--frames" => frames = val("--frames").parse().expect("--frames N"),
            "--seed" => seed = val("--seed").parse().expect("--seed N"),
            "--ops" => ops = Some(PathBuf::from(val("--ops"))),
            "--context" => context = val("--context").parse().expect("--context N"),
            "-h" | "--help" => {
                eprintln!(
                    "usage: trace_diff <trace> [--script FILE] [--quad] [--saver] [--frames N] [--seed N] [--ops FILE] [--context N]"
                );
                return ExitCode::SUCCESS;
            }
            _ if trace_path.is_none() => trace_path = Some(PathBuf::from(a)),
            _ => panic!("unexpected argument {a}"),
        }
    }
    let Some(trace_path) = trace_path else {
        eprintln!("usage: trace_diff <trace> [options]");
        return ExitCode::from(2);
    };
    let ops = ops.unwrap_or_else(|| trace_path.with_file_name("eptrace_ops.json"));
    let frames_data = match trace::load(&ops, &trace_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let mut opts = Options::new(seed);
    opts.quad = quad;
    if saver {
        opts = opts.saver();
    }
    if let Some(s) = script {
        let text = std::fs::read_to_string(&s).unwrap_or_else(|e| panic!("{}: {e}", s.display()));
        opts.script = Some(Script::parse(&text));
    }
    let rep = trace::compare(&frames_data, opts, frames, Tolerance::default());
    println!(
        "{}: {} of {} frames match ({} calls; {} floats, {} not bit-identical, max rel err {:.2e}){}",
        trace_path.display(),
        rep.frames,
        rep.available,
        rep.calls,
        rep.floats.compared,
        rep.floats.inexact,
        rep.floats.max_rel,
        if rep.partial_last {
            "; last frame partial"
        } else {
            ""
        },
    );
    if frames_data.len() > rep.available && rep.available < frames {
        println!(
            "(comparison stops at frame {}: mouse input in the trace)",
            rep.available
        );
    }
    match rep.divergence {
        Some(d) => {
            print!("{}", d.render(context));
            ExitCode::FAILURE
        }
        None => ExitCode::SUCCESS,
    }
}
