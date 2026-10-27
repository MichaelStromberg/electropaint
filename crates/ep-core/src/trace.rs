//! Reading IRIX `eptrace` recordings (EPT1) and comparing them with the
//! simulation's call log.
//!
//! An EPT1 file is the magic `EPT1` followed by records: a big-endian u16
//! opcode, a u16 payload length and the payload, which holds the call's
//! arguments in MIPS (big-endian) order. The opcode table
//! (`eptrace_ops.json`, written by `tools/irix/gen_tracer.py`) gives each
//! opcode's name, parameter types, which pointer parameters were logged by
//! value (and how many floats), output parameters and return type. Frames
//! end at `swapbuffers`.
//!
//! Only the drawing stream is compared (`is_draw_call`): window, event
//! and panel calls are skipped, as is the panel library's own pattern.
//! Window-system REDRAW events are inputs, so the comparison replays them
//! where the trace shows the main loop reading one.

use std::collections::{HashMap, VecDeque};
use std::fmt::{self, Write as _};
use std::path::{Path, PathBuf};

use crate::gl::{Arg, IrisCall};
use crate::sim::{Options, Sim, SwapCause};

// ---------------------------------------------------------------- JSON

/// The few JSON shapes the opcode table uses.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn parse(text: &str) -> Result<Json, String> {
        let mut p = JsonParser {
            s: text.as_bytes(),
            i: 0,
        };
        let v = p.value()?;
        p.ws();
        if p.i != p.s.len() {
            return Err(format!("trailing data at byte {}", p.i));
        }
        Ok(v)
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), String> {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("expected '{}' at byte {}", c as char, self.i))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        self.ws();
        match self.s.get(self.i).copied() {
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.eat(b':')?;
                    m.push((k, self.value()?));
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(m));
                        }
                        _ => return Err(format!("bad object at byte {}", self.i)),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    a.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return Err(format!("bad array at byte {}", self.i)),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Json::Bool(true))
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Json::Bool(false))
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Json::Null)
            }
            Some(_) => {
                let start = self.i;
                while self.i < self.s.len()
                    && matches!(
                        self.s[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[start..self.i]).map_err(|e| e.to_string())?;
                t.parse()
                    .map(Json::Num)
                    .map_err(|_| format!("bad number {t:?} at byte {start}"))
            }
            None => Err("unexpected end".into()),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.s.get(self.i) != Some(&b'"') {
            return Err(format!("expected string at byte {}", self.i));
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i).ok_or("unterminated string")?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|e| e.to_string()),
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or("bad escape")?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let h = self.s.get(self.i..self.i + 4).ok_or("bad escape")?;
                            let h = std::str::from_utf8(h).map_err(|e| e.to_string())?;
                            self.i += 4;
                            let ch = char::from_u32(
                                u32::from_str_radix(h, 16).map_err(|e| e.to_string())?,
                            )
                            .unwrap_or('?');
                            out.extend(ch.to_string().as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                _ => out.push(c),
            }
        }
    }
}

// ---------------------------------------------------------------- opcode table

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ty {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
    Ptr,
}

impl Ty {
    fn parse(s: &str) -> Result<Ty, String> {
        Ok(match s {
            "char" => Ty::I8,
            "unsigned char" => Ty::U8,
            "short" => Ty::I16,
            "unsigned short" => Ty::U16,
            "int" | "long" => Ty::I32,
            "unsigned int" | "unsigned long" => Ty::U32,
            "float" => Ty::F32,
            "double" => Ty::F64,
            s if s.ends_with('*') => Ty::Ptr,
            s => return Err(format!("unknown type {s:?}")),
        })
    }

    fn size(self) -> usize {
        match self {
            Ty::I8 | Ty::U8 => 1,
            Ty::I16 | Ty::U16 => 2,
            Ty::F64 => 8,
            _ => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Extent {
    Fixed(usize),
    /// The float count is the value of another argument.
    Arg(usize),
}

#[derive(Clone, Debug)]
struct OpDef {
    name: &'static str,
    params: Vec<(Ty, Option<Extent>)>,
    outs: usize,
    ret: Option<Ty>,
}

/// The tracer's opcode table.
#[derive(Clone, Debug)]
pub(crate) struct OpsTable {
    ops: HashMap<u16, OpDef>,
}

impl OpsTable {
    pub(crate) fn parse(json: &str) -> Result<OpsTable, String> {
        let j = JsonParser::parse(json)?;
        let Some(Json::Obj(ops)) = j.get("ops") else {
            return Err("no \"ops\" object".into());
        };
        let mut table = HashMap::new();
        for (code, d) in ops {
            let code: u16 = code.parse().map_err(|_| format!("bad opcode {code:?}"))?;
            let name = d
                .get("name")
                .and_then(Json::as_str)
                .ok_or("op without name")?;
            let Some(Json::Arr(params)) = d.get("params") else {
                return Err(format!("{name}: no params"));
            };
            let mut ps = Vec::new();
            for (i, p) in params.iter().enumerate() {
                let ty = Ty::parse(p.as_str().ok_or("bad param")?)?;
                let ext = match d.get("extents").and_then(|e| e.get(&i.to_string())) {
                    Some(Json::Num(n)) => Some(Extent::Fixed(*n as usize)),
                    Some(Json::Str(s)) => {
                        let k = s
                            .strip_prefix("arg")
                            .and_then(|k| k.parse().ok())
                            .ok_or("bad extent")?;
                        Some(Extent::Arg(k))
                    }
                    _ => None,
                };
                ps.push((ty, ext));
            }
            let outs = match d.get("outs") {
                Some(Json::Arr(a)) => a.len(),
                _ => 0,
            };
            let ret = match d.get("ret").and_then(Json::as_str) {
                None | Some("void") => None,
                Some(t) => Some(Ty::parse(t)?),
            };
            table.insert(
                code,
                OpDef {
                    name: static_name(name),
                    params: ps,
                    outs,
                    ret,
                },
            );
        }
        Ok(OpsTable { ops: table })
    }
}

/// `IrisCall` names are `&'static str`. Names the simulation emits are
/// matched to its own literals; any other name is leaked once (a table
/// has about a hundred).
fn static_name(name: &str) -> &'static str {
    DRAW_CALLS
        .iter()
        .copied()
        .find(|n| *n == name)
        .unwrap_or_else(|| Box::leak(name.to_owned().into_boxed_str()))
}

/// One decoded trace record.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Record {
    pub name: &'static str,
    pub args: Vec<Arg>,
    /// The raw values of pointer arguments, in order.
    pub ptrs: Vec<u32>,
    pub ret: Option<Arg>,
}

impl Record {
    /// As a call log entry (`drand48`'s return value becomes its argument).
    pub(crate) fn to_call(&self) -> IrisCall {
        let mut args = self.args.clone();
        if self.name == "drand48" {
            args.extend(self.ret.clone());
        }
        IrisCall {
            name: self.name,
            args,
        }
    }
}

fn be<const N: usize>(p: &[u8], off: usize) -> Option<[u8; N]> {
    p.get(off..off + N)?.try_into().ok()
}

fn read_scalar(ty: Ty, p: &[u8], off: usize) -> Option<Arg> {
    Some(match ty {
        Ty::I8 => Arg::Int(i64::from(i8::from_be_bytes(be(p, off)?))),
        Ty::U8 => Arg::Int(i64::from(u8::from_be_bytes(be(p, off)?))),
        Ty::I16 => Arg::Int(i64::from(i16::from_be_bytes(be(p, off)?))),
        Ty::U16 => Arg::Int(i64::from(u16::from_be_bytes(be(p, off)?))),
        Ty::I32 => Arg::Int(i64::from(i32::from_be_bytes(be(p, off)?))),
        Ty::U32 => Arg::Int(i64::from(u32::from_be_bytes(be(p, off)?))),
        Ty::F32 => Arg::Float(f32::from_be_bytes(be(p, off)?)),
        Ty::F64 => Arg::Double(f64::from_be_bytes(be(p, off)?)),
        Ty::Ptr => Arg::Ptr,
    })
}

/// A streaming EPT1 decoder.
pub(crate) struct Decoder<'a> {
    ops: &'a OpsTable,
    data: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    pub(crate) fn new(ops: &'a OpsTable, data: &'a [u8]) -> Result<Decoder<'a>, String> {
        if !data.starts_with(b"EPT1") {
            return Err("not an EPT1 trace".into());
        }
        Ok(Decoder { ops, data, pos: 4 })
    }

    fn record(&mut self) -> Option<Result<Record, String>> {
        let head = self.data.get(self.pos..self.pos + 4)?;
        let op = u16::from_be_bytes([head[0], head[1]]);
        let len = usize::from(u16::from_be_bytes([head[2], head[3]]));
        let Some(pay) = self.data.get(self.pos + 4..self.pos + 4 + len) else {
            // A truncated final record (the program died mid-write).
            self.pos = self.data.len();
            return None;
        };
        self.pos += 4 + len;
        let Some(d) = self.ops.ops.get(&op) else {
            return Some(Err(format!("unknown opcode {op}")));
        };
        let (mut args, mut ptrs, mut off) = (Vec::with_capacity(d.params.len()), Vec::new(), 0);
        for &(ty, ext) in &d.params {
            let Some((arg, ptr, size)) = read_param(ty, ext, pay, off, &args) else {
                return Some(Err(format!("{}: short payload", d.name)));
            };
            args.push(arg);
            ptrs.extend(ptr);
            off += size;
        }
        off += 4 * d.outs.min(pay.len().saturating_sub(off) / 4);
        let ret = d
            .ret
            .filter(|_| off < pay.len())
            .and_then(|t| read_scalar(if t == Ty::Ptr { Ty::U32 } else { t }, pay, off));
        Some(Ok(Record {
            name: d.name,
            args,
            ptrs,
            ret,
        }))
    }
}

/// One parameter at byte `off` of a payload: the argument, the pointer
/// value if it is a pointer logged by address, and the bytes it takes.
/// `ext` is the extent of a pointer logged by value, which may come from an
/// earlier argument. `None` for a short payload.
fn read_param(
    ty: Ty,
    ext: Option<Extent>,
    pay: &[u8],
    off: usize,
    args: &[Arg],
) -> Option<(Arg, Option<u32>, usize)> {
    if let Some(ext) = ext {
        let n = match ext {
            Extent::Fixed(n) => n,
            Extent::Arg(k) => match args.get(k) {
                Some(Arg::Int(v)) => usize::try_from(*v).unwrap_or(0),
                _ => 0,
            },
        };
        let n = if (1..512).contains(&n) { n } else { 0 };
        let v = (0..n)
            .map(|i| be::<4>(pay, off + 4 * i).map(f32::from_be_bytes))
            .collect::<Option<Vec<f32>>>()?;
        Some((Arg::Floats(v), None, 4 * n))
    } else if ty == Ty::Ptr {
        let p = be::<4>(pay, off).map(u32::from_be_bytes).unwrap_or(0);
        Some((Arg::Ptr, Some(p), 4))
    } else {
        Some((read_scalar(ty, pay, off)?, None, ty.size()))
    }
}

impl Iterator for Decoder<'_> {
    type Item = Result<Record, String>;
    fn next(&mut self) -> Option<Self::Item> {
        self.record()
    }
}

// ---------------------------------------------------------------- filtering

/// Calls that make up the drawing stream (and the random-number stream,
/// which pins down the animation). Everything else ep or libpanel calls is
/// window management, event handling, colour-map bookkeeping or panel
/// drawing into other windows.
const DRAW_CALLS: &[&str] = &[
    "srand48",
    "drand48",
    "doublebuffer",
    "mmode",
    "lmdef",
    "lmbind",
    "RGBmode",
    "blendfunction",
    "subpixel",
    "gconfig",
    "shademodel",
    "concave",
    "glcompat",
    "perspective",
    "loadmatrix",
    "multmatrix",
    "polarview",
    "lsetdepth",
    "reshapeviewport",
    "c4f",
    "cpack",
    "color",
    "clear",
    "zclear",
    "zbuffer",
    "swapbuffers",
    "pushmatrix",
    "popmatrix",
    "rotate",
    "translate",
    "scale",
    "linewidth",
    "n3f",
    "v2f",
    "v3f",
    "t2f",
    "bgnpolygon",
    "endpolygon",
    "bgnclosedline",
    "endclosedline",
    "bgntmesh",
    "endtmesh",
    "frontbuffer",
    "backbuffer",
    "defpattern",
    "setpattern",
    "mapcolor",
];

/// Whether a call belongs to the compared drawing stream. Two names are
/// only partly compared: `mapcolor` only for the five channel swatches ep
/// sets every frame (0x300..0x304; the rest is panel colour-map
/// bookkeeping), and `defpattern` not for libpanel's own pattern 1.
pub(crate) fn is_draw_call(c: &IrisCall) -> bool {
    if !DRAW_CALLS.contains(&c.name) {
        return false;
    }
    match (c.name, c.args.first()) {
        ("mapcolor", Some(Arg::Int(i))) => (0x300..0x305).contains(i),
        ("defpattern", Some(Arg::Int(i))) => *i != 1,
        _ => true,
    }
}

/// One frame of a trace: its drawing calls, whether it ended with a swap,
/// and the input events the main loop saw in it.
#[derive(Clone, Debug, Default)]
pub struct TraceFrame {
    pub calls: Vec<IrisCall>,
    pub complete: bool,
    /// The main loop handled a window-system REDRAW.
    pub redraw: bool,
    /// A mouse button event was read: user input the comparison does not
    /// replay.
    pub mouse: bool,
}

/// Decode a whole trace into frames.
pub(crate) fn frames(ops: &OpsTable, data: &[u8]) -> Result<Vec<TraceFrame>, String> {
    let mut out = Vec::new();
    let mut cur = TraceFrame::default();
    let mut last_qread: Option<i64> = None;
    for r in Decoder::new(ops, data)? {
        let r = r?;
        if r.name == "qread" {
            let v = match r.ret {
                Some(Arg::Int(v)) => v,
                _ => -1,
            };
            if v == 103 {
                cur.mouse = true;
            }
            last_qread = Some(v);
            continue;
        }
        // The main loop's REDRAW handler calls reshapeviewport right after
        // reading the event (fullscreen() calls it after winposition).
        if r.name == "reshapeviewport" && last_qread == Some(528) {
            cur.redraw = true;
        }
        last_qread = None;
        let c = r.to_call();
        if !is_draw_call(&c) {
            continue;
        }
        let swap = c.name == "swapbuffers";
        cur.calls.push(c);
        if swap {
            cur.complete = true;
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.calls.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Why [`load`] failed.
#[derive(Debug)]
pub enum TraceError {
    /// A file could not be read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The opcode table is malformed.
    Table(String),
    /// The trace is not a well-formed EPT1 recording.
    Trace(String),
}

impl fmt::Display for TraceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TraceError::Io { path, source } => write!(f, "{}: {source}", path.display()),
            TraceError::Table(msg) | TraceError::Trace(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for TraceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TraceError::Io { source, .. } => Some(source),
            TraceError::Table(_) | TraceError::Trace(_) => None,
        }
    }
}

/// Load an opcode table and a trace file.
pub fn load(
    ops_json: impl AsRef<Path>,
    trace: impl AsRef<Path>,
) -> Result<Vec<TraceFrame>, TraceError> {
    let (ops_json, trace) = (ops_json.as_ref(), trace.as_ref());
    let text = std::fs::read_to_string(ops_json).map_err(|source| TraceError::Io {
        path: ops_json.into(),
        source,
    })?;
    let ops = OpsTable::parse(&text).map_err(TraceError::Table)?;
    let data = std::fs::read(trace).map_err(|source| TraceError::Io {
        path: trace.into(),
        source,
    })?;
    frames(&ops, &data).map_err(TraceError::Trace)
}

// ---------------------------------------------------------------- comparison

/// Comparison tolerance for floats. Integers (including every `rotate`
/// angle) must match exactly.
#[derive(Clone, Copy, Debug)]
pub struct Tolerance {
    /// Relative tolerance.
    pub rel: f64,
}

impl Default for Tolerance {
    fn default() -> Self {
        Tolerance { rel: 1e-5 }
    }
}

fn close(a: f64, b: f64, tol: Tolerance) -> bool {
    a == b || (a - b).abs() <= tol.rel * a.abs().max(b.abs()).max(1e-6)
}

/// Statistics for the float arguments compared.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FloatStats {
    pub compared: u64,
    /// Floats whose bits differ but are within tolerance (−0 vs +0 counts).
    pub inexact: u64,
    pub max_rel: f64,
}

/// Compare a trace call with a simulated one; `Err` describes the first
/// difference. A NaN on the simulated side matches anything (the original
/// read uninitialised memory there).
pub(crate) fn compare_call(
    trace: &IrisCall,
    sim: &IrisCall,
    tol: Tolerance,
    st: &mut FloatStats,
) -> Result<(), String> {
    if trace.name != sim.name {
        return Err(format!("call {} vs {}", trace.name, sim.name));
    }
    if trace.args.len() != sim.args.len() {
        return Err(format!(
            "{}: {} vs {} args",
            trace.name,
            trace.args.len(),
            sim.args.len()
        ));
    }
    let mut float =
        |i: usize, j: Option<usize>, a: f64, b: f64, same_bits: bool| -> Result<(), String> {
            if b.is_nan() {
                return Ok(());
            }
            st.compared += 1;
            if !same_bits {
                st.inexact += 1;
                if a != b {
                    st.max_rel = st
                        .max_rel
                        .max((a - b).abs() / a.abs().max(b.abs()).max(1e-30));
                }
            }
            if close(a, b, tol) {
                Ok(())
            } else {
                let at = j.map_or(format!("arg {i}"), |j| format!("arg {i}[{j}]"));
                Err(format!("{}: {at}: {a:e} vs {b:e}", trace.name))
            }
        };
    for (i, (a, b)) in trace.args.iter().zip(&sim.args).enumerate() {
        match (a, b) {
            (_, Arg::Ptr) | (Arg::Ptr, _) => {}
            (Arg::Int(x), Arg::Int(y)) => {
                if x != y {
                    return Err(format!("{}: arg {i}: {x} vs {y}", trace.name));
                }
            }
            (Arg::Float(x), Arg::Float(y)) => float(
                i,
                None,
                f64::from(*x),
                f64::from(*y),
                x.to_bits() == y.to_bits(),
            )?,
            (Arg::Double(x), Arg::Double(y)) => float(i, None, *x, *y, x.to_bits() == y.to_bits())?,
            (Arg::Floats(x), Arg::Floats(y)) => {
                if x.len() != y.len() {
                    return Err(format!(
                        "{}: arg {i}: {} vs {} floats",
                        trace.name,
                        x.len(),
                        y.len()
                    ));
                }
                for (j, (p, q)) in x.iter().zip(y).enumerate() {
                    float(
                        i,
                        Some(j),
                        f64::from(*p),
                        f64::from(*q),
                        p.to_bits() == q.to_bits(),
                    )?;
                }
            }
            _ => return Err(format!("{}: arg {i}: type {a:?} vs {b:?}", trace.name)),
        }
    }
    Ok(())
}

/// Where the simulation first left the trace.
#[derive(Clone, Debug)]
pub struct Divergence {
    pub frame: usize,
    /// Index of the differing call within the frame.
    pub call: usize,
    pub message: String,
    /// The frame's calls on both sides, for context.
    pub trace: Vec<IrisCall>,
    pub sim: Vec<IrisCall>,
}

impl Divergence {
    /// A readable excerpt: `context` calls either side of the difference.
    pub fn render(&self, context: usize) -> String {
        let mut s = format!(
            "frame {}, call {}: {}\n",
            self.frame, self.call, self.message
        );
        let lo = self.call.saturating_sub(context);
        for (label, calls) in [("trace", &self.trace), ("sim", &self.sim)] {
            let _ = writeln!(s, "  {label}:");
            for (i, c) in calls.iter().enumerate().skip(lo).take(2 * context + 1) {
                let _ = writeln!(
                    s,
                    "  {} {i:5} {c}",
                    if i == self.call { ">>" } else { "  " }
                );
            }
        }
        s
    }
}

/// The result of comparing a run with a trace.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Frames that matched (a trailing incomplete frame counts once its
    /// recorded prefix matched).
    pub frames: usize,
    /// Trace frames available for comparison.
    pub available: usize,
    pub calls: u64,
    pub floats: FloatStats,
    pub divergence: Option<Divergence>,
    /// A trailing incomplete frame was compared as a prefix.
    pub partial_last: bool,
}

impl Report {
    pub fn passed(&self) -> bool {
        self.divergence.is_none() && self.frames == self.available
    }
}

/// Run the simulation alongside a decoded trace and compare frame by frame,
/// for at most `max_frames` trace frames. REDRAW events are replayed where
/// the trace has them. Comparison stops before the first frame with mouse
/// input.
pub fn compare(
    trace: &[TraceFrame],
    mut opts: Options,
    max_frames: usize,
    tol: Tolerance,
) -> Report {
    opts.call_log = true;
    opts.startup_redraw = false;
    let mut sim = Sim::new(opts);
    let limit = trace
        .iter()
        .position(|f| f.mouse)
        .unwrap_or(trace.len())
        .min(max_frames);
    let mut rep = Report {
        available: limit,
        ..Report::default()
    };
    let mut pending: VecDeque<Vec<IrisCall>> = VecDeque::new();
    let mut cur: Vec<IrisCall> = Vec::new();
    // Frames produced by the simulation so far.
    let mut produced = 0;
    let mut absorb =
        |sim: &mut Sim, pending: &mut VecDeque<Vec<IrisCall>>, produced: &mut usize| {
            for c in sim.take_calls() {
                if !is_draw_call(&c) {
                    continue;
                }
                let swap = c.name == "swapbuffers";
                cur.push(c);
                if swap {
                    pending.push_back(std::mem::take(&mut cur));
                    *produced += 1;
                }
            }
        };
    absorb(&mut sim, &mut pending, &mut produced);
    for f in 0..limit {
        while pending.is_empty() && !sim.exited() {
            if redraw_due(&sim, &trace[..limit], produced) {
                sim.redraw();
            }
            sim.tick();
            absorb(&mut sim, &mut pending, &mut produced);
        }
        let t = &trace[f];
        let s = pending.pop_front().unwrap_or_default();
        let n = if t.complete {
            t.calls.len().max(s.len())
        } else {
            t.calls.len()
        };
        for i in 0..n {
            let res = match (t.calls.get(i), s.get(i)) {
                (Some(a), Some(b)) => compare_call(a, b, tol, &mut rep.floats),
                (Some(a), None) => Err(format!("trace has {a}, sim has nothing")),
                (None, Some(b)) => Err(format!("sim has {b}, trace has nothing")),
                (None, None) => Ok(()),
            };
            if let Err(message) = res {
                rep.divergence = Some(Divergence {
                    frame: f,
                    call: i,
                    message,
                    trace: t.calls.clone(),
                    sim: s,
                });
                return rep;
            }
            rep.calls += 1;
        }
        rep.partial_last |= !t.complete;
        rep.frames += 1;
    }
    rep
}

/// Whether the simulation's next tick should see a REDRAW: the trace reads
/// one in a frame that tick will produce. `produced` frames are already
/// accounted for.
fn redraw_due(sim: &Sim, trace: &[TraceFrame], produced: usize) -> bool {
    let limit = trace.len();
    // A REDRAW belongs to the tick whose frames contain it; a tick
    // produces at most a handful of frames.
    let window = &trace[produced.min(limit)..limit.min(produced + 6)];
    if !window.iter().any(|t| t.redraw) {
        return false;
    }
    let mut probe = sim.clone();
    probe.tick();
    // The startup swap was logged when the simulation was created.
    let n = probe
        .swap_causes()
        .iter()
        .filter(|c| **c != SwapCause::Startup)
        .count();
    trace[produced.min(limit)..limit.min(produced + n)]
        .iter()
        .any(|t| t.redraw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_subset() {
        let j = JsonParser::parse(r#"{"a": [1, -2.5e1, "x\"y"], "b": {"c": null, "d": true}}"#)
            .unwrap();
        assert_eq!(
            j.get("a"),
            Some(&Json::Arr(vec![
                Json::Num(1.0),
                Json::Num(-25.0),
                Json::Str("x\"y".into())
            ]))
        );
        assert_eq!(j.get("b").and_then(|b| b.get("d")), Some(&Json::Bool(true)));
    }

    fn table() -> OpsTable {
        OpsTable::parse(
            r#"{"magic": "EPT1", "ops": {
              "1": {"name": "rotate", "ret": "void", "params": ["short", "char"], "extents": {}, "outs": []},
              "2": {"name": "lmdef", "ret": "void", "params": ["short", "short", "short", "float *"], "extents": {"3": "arg2"}, "outs": []},
              "3": {"name": "drand48", "ret": "double", "params": [], "extents": {}, "outs": []},
              "4": {"name": "swapbuffers", "ret": "void", "params": [], "extents": {}, "outs": []}
            }}"#,
        )
        .unwrap()
    }

    #[test]
    fn decode_records() {
        let ops = table();
        let mut d = b"EPT1".to_vec();
        d.extend([0, 1, 0, 3, 0xff, 0xf6, b'z']);
        d.extend([0, 2, 0, 14, 0, 0, 0, 1, 0, 2]);
        d.extend(1.5f32.to_be_bytes());
        d.extend(2.0f32.to_be_bytes());
        d.extend([0, 3, 0, 8]);
        d.extend(0.25f64.to_be_bytes());
        d.extend([0, 4, 0, 0]);
        let fr = frames(&ops, &d).unwrap();
        assert_eq!(fr.len(), 1);
        let c = &fr[0].calls;
        assert_eq!(
            c[0],
            IrisCall {
                name: "rotate",
                args: vec![Arg::Int(-10), Arg::Int(122)]
            }
        );
        assert_eq!(c[1].args[3], Arg::Floats(vec![1.5, 2.0]));
        assert_eq!(c[2].args, vec![Arg::Double(0.25)]);
        assert!(fr[0].complete);
    }

    #[test]
    fn nan_is_a_wildcard_and_ints_are_exact() {
        let mut st = FloatStats::default();
        let t = IrisCall {
            name: "lmdef",
            args: vec![Arg::Int(0), Arg::Floats(vec![2.0, 2.5e-29])],
        };
        let s = IrisCall {
            name: "lmdef",
            args: vec![Arg::Int(0), Arg::Floats(vec![2.0, f32::NAN])],
        };
        assert!(compare_call(&t, &s, Tolerance::default(), &mut st).is_ok());
        let s2 = IrisCall {
            name: "lmdef",
            args: vec![Arg::Int(1), Arg::Floats(vec![2.0, 0.0])],
        };
        assert!(compare_call(&t, &s2, Tolerance::default(), &mut st).is_err());
    }
}
