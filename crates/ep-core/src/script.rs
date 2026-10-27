//! ep scripts: the command language that drives the actuators
//! (animation.md §1).
//!
//! A script is a list of *sequences*. Each sequence is a time-sorted list of
//! commands; command times are relative to the sequence's start frame. The
//! parser reproduces the original's quirks that change behaviour:
//!
//! * a command's timestamp is the absolute frame **before** its own
//!   `frame:`/`absframe:` update is applied;
//! * new sequences are pushed onto the *front* of the list, and list order
//!   decides both execution order and random-number consumption;
//! * only the first sequence with id 0 (in list order) starts by itself.
//!
//! The built-in "mello" show is provided as data by [`Script::builtin`].

use std::fmt;

/// One script command with its arguments. Actuator and sequence ids are kept
/// as the raw integers the script gave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    /// `frame:`/`relframe:` (a time marker; does nothing when run).
    Frame(f32),
    /// `absframe:` (a time marker).
    AbsFrame(f32),
    /// `actlim1: id, v`: first random-walk limit.
    ActLim1(i32, f32),
    /// `actlim2: id, v`: second limit; also copies the sequence's duration
    /// and variance into the actuator and enables its random walk.
    ActLim2(i32, f32),
    /// `actset: id, v`: set the value (and fire the actuator's callbacks).
    ActSet(i32, f32),
    /// `actreset: id`: value back to its initial value.
    ActReset(i32),
    /// `actresetall:`: the panel's "reset" button.
    ActResetAll,
    /// `actstop: id`: stop that actuator's random walk.
    ActStop(i32),
    /// `actstopall:`: stop every random walk.
    ActStopAll,
    /// `seqdo: id`: (re)start a sequence at the current frame.
    SeqDo(i32),
    /// `seqstop: id`: stop a sequence.
    SeqStop(i32),
    /// `seqloop:`: restart the running sequence, `looplen` frames later.
    SeqLoop,
    /// `seqkill: id`: remove a sequence.
    SeqKill(i32),
    /// `seqkillall:`: remove every sequence.
    SeqKillAll,
    /// `duration: f`: mean segment length for later `actlim2`s.
    Duration(f32),
    /// `variance: f`: randomness of the segment length for later `actlim2`s.
    Variance(f32),
    /// `randdelay: f`: push the sequence start back by a random delay.
    RandDelay(f32),
}

impl Op {
    /// The original's numeric command code (as printed by `ep -o`).
    pub fn code(&self) -> u8 {
        match self {
            Op::Frame(_) => 0,
            Op::AbsFrame(_) => 1,
            Op::ActLim1(..) => 2,
            Op::ActLim2(..) => 3,
            Op::ActSet(..) => 4,
            Op::ActReset(_) => 5,
            Op::ActResetAll => 6,
            Op::ActStop(_) => 7,
            Op::ActStopAll => 8,
            Op::SeqDo(_) => 10,
            Op::SeqStop(_) => 11,
            Op::SeqLoop => 12,
            Op::SeqKill(_) => 14,
            Op::SeqKillAll => 15,
            Op::Duration(_) => 0x65,
            Op::Variance(_) => 0x66,
            Op::RandDelay(_) => 0x67,
        }
    }
}

/// A command and the frame (relative to its sequence's start) at which it
/// becomes due.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Command {
    pub frame: f32,
    pub op: Op,
}

/// A parsed sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct Sequence {
    pub id: i32,
    /// Relative frame after the last command was added; `seqloop:` advances
    /// the start by this much.
    pub looplen: f32,
    /// Commands sorted by frame (stable: ties keep file order).
    pub commands: Vec<Command>,
}

/// A parsed script: sequences in the original's list order (index 0 is the
/// list head, i.e. the most recently created sequence).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    pub sequences: Vec<Sequence>,
    /// The sequence the parser was editing when it finished. Teach mode
    /// (`recordact`) takes its duration from it (the variance is forced
    /// to 1).
    pub edit_seq: Option<i32>,
}

impl Script {
    /// Index (in list order) of the sequence that starts by itself: the
    /// first one with id 0.
    pub fn autostart(&self) -> Option<usize> {
        self.sequences.iter().position(|s| s.id == 0)
    }

    /// Parse script text the way `ep -a` reads it from stdin.
    ///
    /// `-a` reads with `scanf("%[^\n]\n")`, whose trailing `\n` swallows
    /// all whitespace after a line, so blank lines are skipped and leading
    /// whitespace is ignored. Unknown keywords and `#` comments are
    /// dropped; `seqrepeat:` is parsed and dropped ("not implemented").
    /// Arguments that fail to scan stay 0, as the original's calloc'd
    /// command does.
    pub fn parse(text: &str) -> Script {
        let mut p = Parser::default();
        for line in text.split('\n') {
            p.line(line);
        }
        Script {
            sequences: p.seqs,
            edit_seq: p.edit_id,
        }
    }

    /// The built-in show (`ep` / `ep -b`): the "mello" script by Dave
    /// Tristram, as data (animation.md §1.4). Every command is at frame 0
    /// and every `looplen` is 0.
    pub fn builtin() -> Script {
        // (sequence id, commands) in creation order. The list order is the
        // reverse: each new sequence goes to the front.
        let created: Vec<(i32, Vec<Op>)> = vec![
            (
                0,
                vec![
                    Op::ActSet(29, 1.0),     // full screen
                    Op::ActSet(17, 0.0),     // auto off (stops any random walks)
                    Op::ActSet(18, 1.0),     // outline on
                    Op::ActSet(31, f(0.06)), // zoom
                    Op::ActSet(38, 60.0),    // twist
                    Op::ActSet(108, f(1.2)), // size
                    Op::ActSet(52, 40.0),    // strand length n
                    Op::SeqDo(1),
                    Op::SeqDo(2),
                    Op::SeqDo(3),
                    Op::SeqDo(4),
                    Op::SeqDo(5),
                    Op::SeqDo(6),
                    Op::SeqDo(7),
                    Op::SeqDo(8),
                ],
            ),
            // wrist
            (
                1,
                vec![
                    Op::Duration(160.0),
                    Op::ActLim1(101, -1.5),
                    Op::ActLim2(101, 1.5),
                ],
            ),
            // colour of channel 0: stop the hue's self-oscillation, then
            // wander between cyan and green, lightness over the full range
            (
                2,
                vec![
                    Op::Duration(60.0),
                    Op::ActSet(127, 0.0),
                    Op::ActLim1(123, f(0.544053)),
                    Op::ActLim2(123, f(1.295)),
                    Op::Duration(80.0),
                    Op::ActLim1(130, 0.0),
                    Op::ActLim2(130, 1.0),
                ],
            ),
            // wheel
            (
                3,
                vec![
                    Op::Duration(120.0),
                    Op::ActLim1(73, f(0.137)),
                    Op::ActLim2(73, f(-0.137)),
                ],
            ),
            // spin: alternating slow and fast phases
            (
                4,
                vec![
                    Op::Duration(100.0),
                    Op::ActLim1(80, f(0.23)),
                    Op::ActLim2(80, f(-0.23)),
                    Op::RandDelay(1000.0),
                    Op::Duration(40.0),
                    Op::ActLim1(80, f(5.23)),
                    Op::ActLim2(80, f(-5.23)),
                    Op::RandDelay(200.0),
                    Op::SeqLoop,
                ],
            ),
            // flip: alternating slow and fast phases
            (
                5,
                vec![
                    Op::Duration(50.0),
                    Op::ActLim1(87, 2.0),
                    Op::ActLim2(87, -2.0),
                    Op::RandDelay(1200.0),
                    Op::ActLim1(87, 10.0),
                    Op::ActLim2(87, -10.0),
                    Op::RandDelay(220.0),
                    Op::SeqLoop,
                ],
            ),
            // arm
            (
                6,
                vec![
                    Op::Duration(90.0),
                    Op::ActLim1(94, -2.0),
                    Op::ActLim2(94, 2.0),
                ],
            ),
            // twist, very slowly
            (
                7,
                vec![
                    Op::Duration(2250.0),
                    Op::ActLim1(38, 200.0),
                    Op::ActLim2(38, -200.0),
                ],
            ),
            // outline toggle: mostly on
            (
                8,
                vec![
                    Op::Duration(5000.0),
                    Op::ActLim1(18, f(0.2)),
                    Op::ActLim2(18, 1.0),
                ],
            ),
        ];
        let sequences = created
            .into_iter()
            .rev()
            .map(|(id, ops)| Sequence {
                id,
                looplen: 0.0,
                commands: ops
                    .into_iter()
                    .map(|op| Command { frame: 0.0, op })
                    .collect(),
            })
            .collect();
        Script {
            sequences,
            edit_seq: Some(8),
        }
    }
}

/// Decimal to float the way `scanf("%f")` does it here: to double, then
/// narrowed (`const` so the built-in table can use it).
const fn f(x: f64) -> f32 {
    x as f32
}

impl fmt::Display for Script {
    /// The parsed-sequence summary `ep -o` prints after reading a script.
    fn fmt(&self, w: &mut fmt::Formatter<'_>) -> fmt::Result {
        for s in &self.sequences {
            writeln!(w, "seq {} at 0.00", s.id)?;
            for c in &s.commands {
                let (iarg, farg) = op_args(&c.op);
                writeln!(w, "  cmd {},\t{}\t{:.6}", c.op.code(), iarg, farg)?;
            }
        }
        Ok(())
    }
}

/// The (integer, float) argument pair of a command, as stored in the
/// original's command record.
pub(crate) fn op_args(op: &Op) -> (i32, f32) {
    match *op {
        Op::Frame(v) | Op::AbsFrame(v) | Op::Duration(v) | Op::Variance(v) | Op::RandDelay(v) => {
            (0, v)
        }
        Op::ActLim1(i, v) | Op::ActLim2(i, v) | Op::ActSet(i, v) => (i, v),
        Op::ActReset(i) | Op::ActStop(i) | Op::SeqDo(i) | Op::SeqStop(i) | Op::SeqKill(i) => {
            (i, 0.0)
        }
        Op::ActResetAll | Op::ActStopAll | Op::SeqLoop | Op::SeqKillAll => (0, 0.0),
    }
}

#[derive(Default)]
struct Parser {
    seqs: Vec<Sequence>,
    /// Index into `seqs` of the sequence being edited. Indices shift when a
    /// sequence is pushed to the front, so the id is tracked too.
    edit: Option<usize>,
    edit_id: Option<i32>,
    rel: f32,
    base: f32,
    abs: f32,
}

impl Parser {
    fn line(&mut self, raw: &str) {
        let line = raw.trim_start_matches(is_space);
        if line.is_empty() {
            return;
        }
        let end = line.find(is_space).unwrap_or(line.len());
        let (tok, rest) = line.split_at(end);
        // The command's timestamp is taken before this line's own update.
        let frame = self.abs;
        let mut sc = Scan::new(rest);
        let op = match tok {
            "seqname:" => {
                let id = sc.int().unwrap_or(0);
                self.rel = 0.0;
                self.base = 0.0;
                self.abs = 0.0;
                let idx = match self.seqs.iter().position(|s| s.id == id) {
                    Some(i) => i,
                    None => self.push_seq(id),
                };
                self.edit = Some(idx);
                self.edit_id = Some(id);
                return;
            }
            "frame:" | "relframe:" => {
                let v = sc.float().unwrap_or(0.0);
                self.rel += v;
                self.abs = self.rel + self.base;
                Op::Frame(v)
            }
            "absframe:" => {
                let v = sc.float().unwrap_or(0.0);
                self.rel = 0.0;
                self.base = v;
                self.abs = v;
                Op::AbsFrame(v)
            }
            "actlim1:" => {
                let (i, v) = sc.pair();
                Op::ActLim1(i, v)
            }
            "actlim2:" => {
                let (i, v) = sc.pair();
                Op::ActLim2(i, v)
            }
            "actset:" => {
                let (i, v) = sc.pair();
                Op::ActSet(i, v)
            }
            "actreset:" => Op::ActReset(sc.int().unwrap_or(0)),
            "actresetall:" => Op::ActResetAll,
            "actstop:" => Op::ActStop(sc.int().unwrap_or(0)),
            "actstopall:" => Op::ActStopAll,
            "seqdo:" => Op::SeqDo(sc.int().unwrap_or(0)),
            "seqstop:" => Op::SeqStop(sc.int().unwrap_or(0)),
            "seqloop:" => Op::SeqLoop,
            "seqkill:" => Op::SeqKill(sc.int().unwrap_or(0)),
            "seqkillall:" => Op::SeqKillAll,
            "duration:" => Op::Duration(sc.float().unwrap_or(0.0)),
            "variance:" => Op::Variance(sc.float().unwrap_or(0.0)),
            "randdelay:" => Op::RandDelay(sc.float().unwrap_or(0.0)),
            // `seqrepeat:` is "not implemented"; comments and unknown
            // keywords are dropped.
            _ => return,
        };
        let idx = match self.edit {
            Some(i) => i,
            None => {
                let i = self.push_seq(0);
                self.edit = Some(i);
                self.edit_id = Some(0);
                i
            }
        };
        let seq = &mut self.seqs[idx];
        // Stable insertion: before the first command with a strictly
        // greater frame.
        let at = seq
            .commands
            .iter()
            .position(|c| frame < c.frame)
            .unwrap_or(seq.commands.len());
        seq.commands.insert(at, Command { frame, op });
        seq.looplen = self.rel;
    }

    /// Push a new sequence to the list front; returns its index (0).
    fn push_seq(&mut self, id: i32) -> usize {
        self.seqs.insert(
            0,
            Sequence {
                id,
                looplen: 0.0,
                commands: Vec::new(),
            },
        );
        if let Some(e) = self.edit.as_mut() {
            *e += 1;
        }
        0
    }
}

/// C `isspace`.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// A tiny `sscanf` for the three formats the parser uses: `%d`, `%f` and
/// `%d, %f`.
struct Scan<'a> {
    s: &'a str,
}

impl<'a> Scan<'a> {
    fn new(s: &'a str) -> Self {
        Scan { s }
    }

    fn skip_space(&mut self) {
        self.s = self.s.trim_start_matches(is_space);
    }

    /// `%d`: optional sign and decimal digits (after skipping whitespace).
    fn int(&mut self) -> Option<i32> {
        self.skip_space();
        let b = self.s.as_bytes();
        let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
        let digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        i += digits;
        let v = self.s[..i].parse::<i64>().ok()? as i32;
        self.s = &self.s[i..];
        Some(v)
    }

    /// `%f`: a `strtod`-style decimal (sign, digits, fraction, exponent),
    /// converted to double and then stored as float.
    fn float(&mut self) -> Option<f32> {
        self.skip_space();
        let b = self.s.as_bytes();
        let mut i = usize::from(matches!(b.first(), Some(b'+' | b'-')));
        let int_digits = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
        i += int_digits;
        let mut frac_digits = 0;
        if b.get(i) == Some(&b'.') {
            frac_digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            if int_digits + frac_digits > 0 {
                i += 1 + frac_digits;
            }
        }
        if int_digits + frac_digits == 0 {
            return None;
        }
        if matches!(b.get(i), Some(b'e' | b'E')) {
            let mut j = i + 1;
            if matches!(b.get(j), Some(b'+' | b'-')) {
                j += 1;
            }
            let exp_digits = b[j.min(b.len())..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            if exp_digits > 0 {
                i = j + exp_digits;
            }
        }
        let v = self.s[..i].parse::<f64>().ok()? as f32;
        self.s = &self.s[i..];
        Some(v)
    }

    /// `%d, %f`. The comma must follow the number immediately; the space in
    /// the format matches any amount of whitespace. Fields that fail keep 0.
    fn pair(&mut self) -> (i32, f32) {
        let Some(i) = self.int() else { return (0, 0.0) };
        let Some(rest) = self.s.strip_prefix(',') else {
            return (i, 0.0);
        };
        self.s = rest;
        (i, self.float().unwrap_or(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_shape() {
        let s = Script::builtin();
        let ids: Vec<i32> = s.sequences.iter().map(|q| q.id).collect();
        assert_eq!(ids, [8, 7, 6, 5, 4, 3, 2, 1, 0]);
        assert_eq!(
            s.sequences.iter().map(|q| q.commands.len()).sum::<usize>(),
            54
        );
        assert_eq!(s.autostart(), Some(8));
    }

    #[test]
    fn timestamps_and_order() {
        let s = Script::parse(
            "actset: 1, 2.5\nframe: 10\nactset: 2, 1\n  # comment\n\nseqname: 3\nframe: 5\nduration: 7\nabsframe: 2\nseqloop:\n",
        );
        assert_eq!(s.sequences.len(), 2);
        assert_eq!(s.sequences[0].id, 3);
        let s0 = &s.sequences[1];
        assert_eq!(s0.id, 0);
        // `frame:` lines are stored; each command's frame is the time
        // before its own line's update.
        let frames: Vec<f32> = s0.commands.iter().map(|c| c.frame).collect();
        assert_eq!(frames, [0.0, 0.0, 10.0]);
        assert_eq!(s0.looplen, 10.0);
        assert_eq!(s0.commands[2].op, Op::ActSet(2, 1.0));
        let s3 = &s.sequences[0];
        // seqloop's frame is the absframe value; insertion is stable by
        // frame, so it lands between the frame: and duration: commands.
        let ops: Vec<(f32, u8)> = s3.commands.iter().map(|c| (c.frame, c.op.code())).collect();
        assert_eq!(ops, [(0.0, 0), (2.0, 12), (5.0, 0x65), (5.0, 1)]);
        assert_eq!(s3.looplen, 0.0);
        assert_eq!(s.edit_seq, Some(3));
    }

    #[test]
    fn scanf_quirks() {
        let s = Script::parse(
            "actset: 5 , 1.0\nactlim1: 7,  -2.5e1 \nduration: abc\nrandom: 3\nseqrepeat: 2\n",
        );
        let ops: Vec<Op> = s.sequences[0].commands.iter().map(|c| c.op).collect();
        assert_eq!(
            ops,
            [Op::ActSet(5, 0.0), Op::ActLim1(7, -25.0), Op::Duration(0.0)]
        );
    }

    #[test]
    fn existing_sequence_is_reopened() {
        let s = Script::parse(
            "seqname: 1\nactstop: 3\nseqname: 2\nactstop: 4\nseqname: 1\nactstop: 5\n",
        );
        let ids: Vec<i32> = s.sequences.iter().map(|q| q.id).collect();
        assert_eq!(ids, [2, 1]);
        assert_eq!(s.sequences[1].commands.len(), 2);
        assert_eq!(s.autostart(), None);
    }

    /// `re/scripts/mello.ep` is the built-in script as text (git-ignored;
    /// skipped when absent). Parsing it must give the built-in data.
    #[test]
    fn mello_text_matches_builtin() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/scripts/mello.ep");
        let Ok(text) = std::fs::read_to_string(path) else {
            return crate::testing::re_missing(path);
        };
        assert_eq!(Script::parse(&text), Script::builtin());
    }

    #[test]
    fn variant_scripts_parse() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../re/scripts");
        let Ok(entries) = std::fs::read_dir(dir) else {
            return crate::testing::re_missing(dir);
        };
        let base = Script::builtin();
        for e in entries.flatten() {
            let name = e.file_name().into_string().unwrap();
            if !name.starts_with("mello-") || !name.ends_with(".ep") {
                continue;
            }
            let s = Script::parse(&std::fs::read_to_string(e.path()).unwrap());
            // Same sequences; seq 0 has extra `actset: id, 1.0` lines
            // between its 7 actsets and its seqdos.
            assert_eq!(s.sequences.len(), base.sequences.len(), "{name}");
            assert_eq!(s.sequences[..8], base.sequences[..8], "{name}");
            let s0 = &s.sequences[8].commands;
            let k = s0.len() - base.sequences[8].commands.len();
            assert!(k > 0, "{name}");
            assert_eq!(s0[..7], base.sequences[8].commands[..7], "{name}");
            assert_eq!(s0[7 + k..], base.sequences[8].commands[7..], "{name}");
            assert!(
                s0[7..7 + k]
                    .iter()
                    .all(|c| matches!(c.op, Op::ActSet(_, v) if v == 1.0)),
                "{name}"
            );
        }
    }
}
