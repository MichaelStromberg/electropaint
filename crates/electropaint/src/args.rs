//! The command line: what it asks for, and its usage text.

use ep_metal::surface::{Strands, Style};

/// What the command line asks for.
pub(crate) struct Options {
    pub(crate) windowed: bool,
    pub(crate) all_displays: bool,
    pub(crate) controls: bool,
    /// Hidden: `--panel-snapshot FILE` draws the control panel to PNGs.
    pub(crate) panel_snapshot: Option<String>,
    pub(crate) quad: bool,
    pub(crate) script: Option<ep_core::Script>,
    pub(crate) seed: Option<i64>,
    pub(crate) snapshot: Option<Snapshot>,
    /// `None` for the style's own default: one strand in classic, as many
    /// as fit in remastered.
    pub(crate) strands: Option<Strands>,
    pub(crate) style: Style,
}

impl Options {
    /// The options for each show: ep's, with this run's flags and script.
    pub(crate) fn show_options(&self, seed: i64) -> ep_core::Options {
        let mut o = ep_core::Options::new(seed);
        // Remastered draws from scenes; recording them always lets the style
        // change while the show runs.
        o.scene = true;
        o.quad = self.quad;
        if let Some(script) = &self.script {
            o.script = Some(script.clone());
        }
        o
    }

    /// The first show's seed: `--seed`, or the clock as the original does.
    pub(crate) fn seed_base(&self) -> i64 {
        self.seed.unwrap_or_else(ep_core::Options::clock_seed)
    }

    pub(crate) fn strands(&self) -> Strands {
        self.strands.unwrap_or_else(|| self.style.strands())
    }
}

/// The most strands `--strands` takes: each has buffers of its own.
const MAX_STRANDS: usize = 16;

/// `--snapshot FILE`: render offscreen and write the picture shown after the
/// given number of 60 Hz retraces as a PNG, then exit.
pub(crate) struct Snapshot {
    pub(crate) path: String,
    pub(crate) frames: u64,
    pub(crate) width: usize,
    pub(crate) height: usize,
}

pub(crate) fn parse_args() -> Options {
    match parse(std::env::args().skip(1)) {
        Ok(Some(opts)) => opts,
        Ok(None) => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        Err(msg) => {
            eprintln!("electropaint: {msg}\n{USAGE}");
            std::process::exit(2);
        }
    }
}

/// The options `args` asks for, `None` for `--help`, or what is wrong with
/// them.
fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, String> {
    let mut opts = Options {
        windowed: false,
        all_displays: false,
        controls: false,
        panel_snapshot: None,
        quad: false,
        script: None,
        seed: None,
        snapshot: None,
        strands: None,
        style: Style::Classic,
    };
    // The snapshot's file, steps and size, in any order.
    let (mut snapshot, mut frames, mut size) = (None, None, None);
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--windowed" => opts.windowed = true,
            "--all-displays" => opts.all_displays = true,
            "-c" | "--controls" => opts.controls = true,
            "--panel-snapshot" => {
                opts.panel_snapshot = Some(args.next().ok_or("--panel-snapshot needs a file")?);
            }
            "-q" | "--quad" => opts.quad = true,
            "--script" => {
                let path = args.next().ok_or("--script needs a file")?;
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
                opts.script = Some(ep_core::Script::parse(&text));
            }
            "--seed" => opts.seed = Some(number(args.next(), "--seed")?),
            "--snapshot" => snapshot = Some(args.next().ok_or("--snapshot needs a file")?),
            "--frames" => frames = Some(number(args.next(), "--frames")?),
            "--strands" => {
                let v = args.next().unwrap_or_default();
                opts.strands = Some(if v == "auto" {
                    Strands::Auto
                } else {
                    match v.parse() {
                        Ok(n @ 1..=MAX_STRANDS) => Strands::Count(n),
                        _ => return Err(format!("--strands takes auto or 1 to {MAX_STRANDS}")),
                    }
                });
            }
            "--style" => {
                opts.style = match args.next().as_deref() {
                    Some("classic") => Style::Classic,
                    Some("remastered") => Style::Remastered,
                    _ => return Err("--style takes classic or remastered".into()),
                };
            }
            "--size" => {
                let v = args.next().unwrap_or_default();
                size = Some(
                    v.split_once('x')
                        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                        .filter(|&(w, h)| w > 0 && h > 0)
                        .ok_or("--size takes WIDTHxHEIGHT, both above 0")?,
                );
            }
            "-h" | "--help" => return Ok(None),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if opts.windowed && opts.all_displays {
        return Err("--windowed and --all-displays don't go together".into());
    }
    match snapshot {
        Some(path) => {
            if opts.windowed || opts.all_displays || opts.controls || opts.panel_snapshot.is_some()
            {
                return Err("--snapshot draws offscreen, with no windows or control panel".into());
            }
            let (width, height) = size.unwrap_or((1280, 1024));
            opts.snapshot = Some(Snapshot {
                path,
                frames: frames.unwrap_or(600),
                width,
                height,
            });
        }
        None if frames.is_some() || size.is_some() => {
            return Err("--frames and --size go with --snapshot".into());
        }
        None => {}
    }
    Ok(Some(opts))
}

const USAGE: &str = "usage: electropaint [--windowed | --all-displays] [-c | --controls] [--style classic|remastered] [-q | --quad] [--script FILE] [--seed N] [--strands N|auto]
       electropaint --snapshot FILE.png [--style classic|remastered] [-q | --quad] [--script FILE] [--seed N] [--strands N|auto] [--frames N] [--size WxH]
       electropaint -h | --help

--style remastered draws ep's show as iridescent chrome in high dynamic
range, as many strands as fit the screen; classic (the default) as ep drew it.
--script runs an ep script (as `ep -a`) instead of the built-in show.
--strands runs several shows side by side (up to 16), each with its own
seed; auto fits as many as the screen's width takes (three on 32:9).
--snapshot saves the picture after about --frames 60 Hz retraces (default 600)
at --size (default 1280x1024).

Fullscreen on the main display (every display with --all-displays, each
with its own show); any key or click quits.

With --windowed or --controls, the show is interactive as ep was: keys are
its shortcuts (R ribbons, Z z-buffer, L light, Q smear, ...), a click
toggles ep's full-screen state, and leaving full screen brings up the
control panel (--controls keeps it up). Esc quits.

The show steps at the original's 60 Hz; faster displays get in-between
pictures on the refreshes between steps.";

/// An option's numeric value, if it has one.
fn number<T: std::str::FromStr>(v: Option<String>, what: &str) -> Result<T, String> {
    v.and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("{what} needs a number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(args: &str) -> Result<Option<Options>, String> {
        parse(args.split_whitespace().map(String::from))
    }

    fn snapshot(args: &str) -> Option<Snapshot> {
        parsed(args).ok().flatten().and_then(|o| o.snapshot)
    }

    #[test]
    fn snapshot_takes_the_frame_and_size_options_in_any_order() {
        let s = snapshot("--snapshot a.png").expect("a snapshot");
        assert_eq!((s.frames, s.width, s.height), (600, 1280, 1024));
        let s = snapshot("--frames 5 --snapshot a.png --size 64x32").expect("a snapshot");
        assert_eq!(
            (s.path.as_str(), s.frames, s.width, s.height),
            ("a.png", 5, 64, 32)
        );
    }

    #[test]
    fn options_and_values() {
        let o = parsed("-c --quad --seed -3 --windowed").ok().flatten();
        let o = o.expect("options");
        assert!(o.controls && o.quad && o.windowed && !o.all_displays);
        assert_eq!(o.seed, Some(-3));
        assert!(o.snapshot.is_none() && o.panel_snapshot.is_none());
        assert!(matches!(parsed("--help"), Ok(None)));
        let strands = |args| parsed(args).ok().flatten().map(|o| o.strands());
        assert_eq!(strands(""), Some(Strands::Count(1)));
        assert_eq!(strands("--strands 3"), Some(Strands::Count(3)));
        assert_eq!(strands("--strands auto"), Some(Strands::Auto));
        assert_eq!(strands("--style remastered"), Some(Strands::Auto));
        assert_eq!(
            strands("--style remastered --strands 2"),
            Some(Strands::Count(2))
        );
    }

    #[test]
    fn malformed_options_are_errors() {
        for args in [
            "--seed",
            "--seed x",
            "--frames -1",
            "--size",
            "--size 64",
            "--size 0x5",
            "--size 5xa",
            "--snapshot",
            "--panel-snapshot",
            "--script",
            "--fast",
            "--windowed --all-displays",
            "--frames 5",
            "--size 64x32",
            "--snapshot a.png --windowed",
            "--snapshot a.png --controls",
            "--strands",
            "--strands 0",
            "--strands 17",
            "--strands wide",
            "--style",
            "--style modern",
        ] {
            assert!(parsed(args).is_err(), "{args} should be refused");
        }
        assert_eq!(
            parsed("--fast").err().as_deref(),
            Some("unknown option --fast")
        );
    }
}
