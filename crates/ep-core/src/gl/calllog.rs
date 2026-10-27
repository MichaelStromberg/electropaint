//! The call log: IrisGL calls as the IRIX tracer records them.

/// A logged argument.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    /// Any integer type (`short`, `long`, `char`, colour indices, tokens).
    Int(i64),
    Float(f32),
    /// A pointer argument logged by value (`c4f`, `v2f`, `lmdef`, …).
    /// A NaN element marks a value the original reads from uninitialised
    /// memory; comparisons accept anything there.
    Floats(Vec<f32>),
    /// A double return value (`drand48`).
    Double(f64),
    /// A pointer whose contents are not logged.
    Ptr,
}

/// One IrisGL (or libc) call as the IRIX tracer records it. Return values
/// that matter (only `drand48`'s) are appended to `args`.
#[derive(Clone, Debug, PartialEq)]
pub struct IrisCall {
    pub name: &'static str,
    pub args: Vec<Arg>,
}

impl std::fmt::Display for IrisCall {
    fn fmt(&self, w: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(w, "{}(", self.name)?;
        for (i, a) in self.args.iter().enumerate() {
            if i > 0 {
                write!(w, ", ")?;
            }
            match a {
                Arg::Int(v) => write!(w, "{v}")?,
                Arg::Float(v) => write!(w, "{v:e}")?,
                Arg::Double(v) => write!(w, "{v:e}")?,
                Arg::Ptr => write!(w, "ptr")?,
                Arg::Floats(v) => {
                    write!(w, "[")?;
                    for (j, x) in v.iter().enumerate() {
                        write!(w, "{}{x:e}", if j > 0 { " " } else { "" })?;
                    }
                    write!(w, "]")?;
                }
            }
        }
        write!(w, ")")
    }
}
