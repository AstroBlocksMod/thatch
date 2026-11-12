//! The Thatch IR: what the frontends produce and the backends consume.
//!
//! It is a structured, typed representation. Every expression has a static type ([`Type`]), every
//! conversion between types is an explicit [`Expr::Cast`], loops are real loops with explicit
//! [`Stmt::Yield`] points where the cooperative scheduler may switch threads, and everything that
//! touches the outside world is a call into the runtime ([`Expr::Rt`] and [`Stmt::Rt`]). Nothing in
//! here knows what a Scratch block is: the Scratch frontend lowers blocks into this, and a Snap!
//! frontend would lower into the same thing.
//!
//! Values of type [`Type::Any`] are dynamically typed Scratch values (number, string or boolean) kept
//! in runtime storage. [`Type::Num`], [`Type::Str`] and [`Type::Bool`] are statically known and live in
//! registers (numbers and booleans) or as string handles owned by the running thread.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    Num,
    Str,
    Bool,
    Any,
}

/// A string in the program's constant pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StrId(pub u32);

/// A function in [`Program::functions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

/// A variable, as the generated code addresses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarRef {
    /// A numeric variable of the thread's own target.
    TargetNum(u32),
    /// A dynamically typed variable of the thread's own target.
    TargetAny(u32),
    /// A numeric variable of the stage (a global).
    StageNum(u32),
    /// A dynamically typed variable of the stage.
    StageAny(u32),
    /// A parameter of the current function.
    Param(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    /// Scratch's `mod`: the result takes the sign of the divisor.
    Mod,
    /// Comparisons take two operands of any type; see [`Expr::ty`].
    Lt,
    Gt,
    Eq,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    /// `Math.round`.
    Round,
    Abs,
    Floor,
    Ceil,
    Sqrt,
    /// The `mathop` block's other operations, by their index in [`MATH_OPS`].
    MathOp(u8),
}

/// The `mathop` menu entries that [`UnOp::MathOp`] indexes, in the runtime's order.
pub const MATH_OPS: &[&str] = &["sin", "cos", "tan", "asin", "acos", "atan", "ln", "log", "e ^", "10 ^"];

/// Runtime reporters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtExpr {
    /// `join a b`: Str, Str -> Str.
    Join,
    /// `letter n of s`: Num, Str -> Str.
    LetterOf,
    /// `length of s`: Str -> Num.
    Length,
    /// `s contains part`: Str, Str -> Bool.
    Contains,
    /// `pick random a to b`: Any, Any -> Num. The raw values decide whether the result is an integer.
    Random,
    /// `timer`: -> Num.
    Timer,
}

/// Runtime commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtStmt {
    /// `say v` (Any). The runtime rounds numbers to two decimals, as Scratch shows them.
    Say,
    /// `think v` (Any).
    Think,
    /// `say v for secs` (Any, Num). Yields until the time is up.
    SayFor,
    /// `think v for secs` (Any, Num).
    ThinkFor,
    /// `wait secs` (Num). Yields until the time is up.
    Wait,
    /// `broadcast name` (Str).
    Broadcast,
    /// `broadcast name and wait` (Str). Yields until the started threads finish.
    BroadcastAndWait,
    /// `stop all`: ends every thread, including this one.
    StopAll,
    /// `stop other scripts in sprite`.
    StopOthers,
    /// `reset timer`.
    ResetTimer,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Str(StrId),
    Bool(bool),
    Var(VarRef),
    Cast(Type, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Rt(RtExpr, Vec<Expr>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// Stores the value in the variable. The expression's type must match a typed variable's type;
    /// an `Any` variable takes an expression of any type.
    Set(VarRef, Expr),
    If {
        cond: Expr,
        then: Vec<Stmt>,
        els: Vec<Stmt>,
    },
    /// A counted loop. `times` (Num) is rounded once; each iteration first decrements the counter and
    /// runs the body only while the counter is still at least 0 (so NaN runs it never, infinity
    /// forever). The body carries its own `Yield`.
    Repeat {
        times: Expr,
        body: Vec<Stmt>,
    },
    /// An endless loop that only `Break` leaves.
    Loop(Vec<Stmt>),
    Break,
    /// The end of a loop iteration in cooperative code: lets other threads run, unless the thread is
    /// in warp mode.
    Yield,
    /// Calls a function with arguments matching its parameter types. `nesting` is how many loops and
    /// conditionals enclose the call within its function, which the runtime's recursion rule needs.
    Call {
        func: FuncId,
        args: Vec<Expr>,
        nesting: u32,
    },
    Rt(RtStmt, Vec<Expr>),
    /// Leaves the function. In a script, that ends the thread.
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    pub name: String,
    pub params: Vec<Param>,
    /// Runs without yielding at loop ends ("run without screen refresh").
    pub warp: bool,
    pub body: Vec<Stmt>,
}

/// What starts a script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hat {
    FlagClicked,
    /// The message name, matched case-insensitively.
    BroadcastReceived(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    pub hat: Hat,
    pub func: FuncId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NumVar {
    pub name: String,
    pub initial: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Num(f64),
    Str(StrId),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AnyVar {
    pub name: String,
    pub initial: Const,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    pub is_stage: bool,
    pub num_vars: Vec<NumVar>,
    pub any_vars: Vec<AnyVar>,
    pub scripts: Vec<Script>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Program {
    /// The stage first, then the sprites in layer order.
    pub targets: Vec<Target>,
    pub strings: Vec<String>,
    pub functions: Vec<Function>,
}

impl Program {
    /// Adds a string to the constant pool, reusing an equal one.
    pub fn intern(&mut self, s: &str) -> StrId {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return StrId(i as u32);
        }
        self.strings.push(s.to_owned());
        StrId((self.strings.len() - 1) as u32)
    }

    pub fn add_function(&mut self, f: Function) -> FuncId {
        self.functions.push(f);
        FuncId((self.functions.len() - 1) as u32)
    }
}

impl Expr {
    /// The static type of the expression. Variable and parameter types come from `vars`.
    pub fn ty(&self, vars: &dyn Fn(VarRef) -> Type) -> Type {
        match self {
            Expr::Num(_) => Type::Num,
            Expr::Str(_) => Type::Str,
            Expr::Bool(_) => Type::Bool,
            Expr::Var(v) => vars(*v),
            Expr::Cast(t, _) => *t,
            Expr::Unary(op, _) => match op {
                UnOp::Not => Type::Bool,
                _ => Type::Num,
            },
            Expr::Binary(op, _, _) => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => Type::Num,
                BinOp::Lt | BinOp::Gt | BinOp::Eq | BinOp::And | BinOp::Or => Type::Bool,
            },
            Expr::Rt(f, _) => match f {
                RtExpr::Join | RtExpr::LetterOf => Type::Str,
                RtExpr::Length | RtExpr::Random | RtExpr::Timer => Type::Num,
                RtExpr::Contains => Type::Bool,
            },
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Type::Num => "num",
            Type::Str => "str",
            Type::Bool => "bool",
            Type::Any => "any",
        })
    }
}

/// Writes the program in a readable text form, for tests and `thatch dump`.
pub fn dump(program: &Program) -> String {
    let mut out = String::new();
    for (i, s) in program.strings.iter().enumerate() {
        out.push_str(&format!("str {i} = {s:?}\n"));
    }
    for t in &program.targets {
        out.push_str(&format!("target {:?}{}\n", t.name, if t.is_stage { " (stage)" } else { "" }));
        for (i, v) in t.num_vars.iter().enumerate() {
            out.push_str(&format!("  num {i} {:?} = {}\n", v.name, v.initial));
        }
        for (i, v) in t.any_vars.iter().enumerate() {
            out.push_str(&format!("  any {i} {:?} = {:?}\n", v.name, v.initial));
        }
        for s in &t.scripts {
            out.push_str(&format!("  script {:?} -> fn {}\n", s.hat, s.func.0));
        }
    }
    for (i, f) in program.functions.iter().enumerate() {
        let params: Vec<String> = f.params.iter().map(|p| format!("{}: {}", p.name, p.ty)).collect();
        out.push_str(&format!("fn {i} {:?}({}){}\n", f.name, params.join(", "), if f.warp { " warp" } else { "" }));
        dump_block(&mut out, &f.body, 1);
    }
    out
}

fn dump_block(out: &mut String, stmts: &[Stmt], depth: usize) {
    let pad = "  ".repeat(depth);
    for s in stmts {
        match s {
            Stmt::Set(v, e) => out.push_str(&format!("{pad}set {v:?} = {}\n", dump_expr(e))),
            Stmt::If { cond, then, els } => {
                out.push_str(&format!("{pad}if {}\n", dump_expr(cond)));
                dump_block(out, then, depth + 1);
                if !els.is_empty() {
                    out.push_str(&format!("{pad}else\n"));
                    dump_block(out, els, depth + 1);
                }
            }
            Stmt::Repeat { times, body } => {
                out.push_str(&format!("{pad}repeat {}\n", dump_expr(times)));
                dump_block(out, body, depth + 1);
            }
            Stmt::Loop(body) => {
                out.push_str(&format!("{pad}loop\n"));
                dump_block(out, body, depth + 1);
            }
            Stmt::Break => out.push_str(&format!("{pad}break\n")),
            Stmt::Yield => out.push_str(&format!("{pad}yield\n")),
            Stmt::Call { func, args, nesting } => {
                let args: Vec<String> = args.iter().map(dump_expr).collect();
                out.push_str(&format!("{pad}call fn {}({}) nesting {nesting}\n", func.0, args.join(", ")));
            }
            Stmt::Rt(f, args) => {
                let args: Vec<String> = args.iter().map(dump_expr).collect();
                out.push_str(&format!("{pad}{f:?}({})\n", args.join(", ")));
            }
            Stmt::Return => out.push_str(&format!("{pad}return\n")),
        }
    }
}

fn dump_expr(e: &Expr) -> String {
    match e {
        Expr::Num(n) => format!("{n}"),
        Expr::Str(s) => format!("str {}", s.0),
        Expr::Bool(b) => format!("{b}"),
        Expr::Var(v) => format!("{v:?}"),
        Expr::Cast(t, e) => format!("({t}){}", dump_expr(e)),
        Expr::Unary(op, e) => format!("{op:?}({})", dump_expr(e)),
        Expr::Binary(op, a, b) => format!("{op:?}({}, {})", dump_expr(a), dump_expr(b)),
        Expr::Rt(f, args) => {
            let args: Vec<String> = args.iter().map(dump_expr).collect();
            format!("{f:?}({})", args.join(", "))
        }
    }
}
