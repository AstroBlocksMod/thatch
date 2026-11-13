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
    /// `Cast.toNumber` on a number: NaN becomes 0. The frontend applies it wherever a block
    /// consumes a numeric input, since `0 / 0` is NaN but `(0 / 0) + 1` is 1 in Scratch.
    ToNumber,
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
    /// The sprite's x position: -> Num. Values within a billionth of a whole number are rounded.
    SpriteX,
    /// The sprite's y position: -> Num, rounded the same way.
    SpriteY,
    /// The sprite's direction: -> Num.
    SpriteDirection,
    /// The sprite's size, rounded to a whole number: -> Num.
    SpriteSize,
    /// The number of the current costume, counted from 1: -> Num.
    CostumeNumber,
    /// The name of the current costume: -> Str.
    CostumeName,
    /// The number of the stage's current backdrop, counted from 1: -> Num.
    BackdropNumber,
    /// The name of the stage's current backdrop: -> Str.
    BackdropName,
    /// `property of object`: Str, Str -> Any. The property is a name such as "x position" or a
    /// variable name; the object a sprite name or "_stage_".
    PropertyOf,
    /// `mouse x`: -> Num.
    MouseX,
    /// `mouse y`: -> Num.
    MouseY,
    /// `mouse down?`: -> Bool.
    MouseDown,
    /// `key pressed?`: Any -> Bool. The key is a name ("space", "a", "any"), a letter or a key code.
    KeyPressed,
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

    // Motion. A target name (Str) is a sprite's name, "_mouse_" or "_random_".
    /// `move steps` (Num).
    MoveSteps,
    /// `go to x: y:` (Num, Num).
    GoToXY,
    /// `go to target` (Str).
    GoTo,
    /// `set x to` (Num).
    SetX,
    /// `set y to` (Num).
    SetY,
    /// `change x by` (Num).
    ChangeX,
    /// `change y by` (Num).
    ChangeY,
    /// `turn right degrees` (Num). Turning left is a turn by the negated amount.
    Turn,
    /// `point in direction` (Num).
    PointInDirection,
    /// `point towards target` (Str).
    PointTowards,
    /// `glide secs to x: y:` (Num, Num, Num). Yields until the time is up.
    Glide,
    /// `glide secs to target` (Num, Str). Yields until the time is up.
    GlideTo,
    /// `if on edge, bounce`.
    Bounce,
    /// `set rotation style` to the given [`RotationStyle`].
    SetRotationStyle(RotationStyle),

    // Looks.
    /// `show`.
    Show,
    /// `hide`.
    Hide,
    /// `switch costume to` (Any): a number is a costume number, a string a costume name first.
    SwitchCostume,
    /// `next costume`.
    NextCostume,
    /// `switch backdrop to` (Any), on the stage, from any target.
    SwitchBackdrop,
    /// `next backdrop`.
    NextBackdrop,
    /// `set size to` (Num), in percent.
    SetSize,
    /// `change size by` (Num).
    ChangeSize,
    /// `set effect to` (Num), for the given effect.
    SetEffect(Effect),
    /// `change effect by` (Num).
    ChangeEffect(Effect),
    /// `clear graphic effects`.
    ClearEffects,
    /// `go to front layer` or, with `false`, `go to back layer`.
    GoToLayer(bool),
    /// `go forward layers` (Num). Going backward is going forward by the negated amount.
    GoLayers,
}

/// How a sprite's direction shows when it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationStyle {
    AllAround,
    LeftRight,
    DontRotate,
}

/// The graphic effects, in the order the runtime and the ABI keep them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Color,
    Fisheye,
    Whirl,
    Pixelate,
    Mosaic,
    Brightness,
    Ghost,
}

impl Effect {
    pub const ALL: [Effect; 7] = [
        Effect::Color,
        Effect::Fisheye,
        Effect::Whirl,
        Effect::Pixelate,
        Effect::Mosaic,
        Effect::Brightness,
        Effect::Ghost,
    ];

    /// The effect's position in [`Effect::ALL`], which is also its index in the ABI.
    pub fn index(self) -> u8 {
        Effect::ALL.iter().position(|e| *e == self).unwrap_or(0) as u8
    }
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
    /// `when key pressed`, with the key as the menu names it ("space", "left arrow", "a", "0" or
    /// "any"), matched case-insensitively. A key hat does not restart its script while a thread of
    /// it is still running.
    KeyPressed(String),
    /// `when this sprite clicked`.
    SpriteClicked,
    /// `when stage clicked`. Like the sprite hat, it fires when the target it sits on is clicked.
    StageClicked,
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

/// How a target starts out on the stage. For the stage itself only `costume` (the backdrop) and
/// `volume` matter.
#[derive(Debug, Clone, PartialEq)]
pub struct SpriteState {
    pub x: f64,
    pub y: f64,
    /// -179 to 180, 90 pointing right.
    pub direction: f64,
    /// Percent of the costume's size.
    pub size: f64,
    /// The index of the current costume, counted from 0.
    pub costume: u32,
    pub visible: bool,
    pub rotation_style: RotationStyle,
    /// Whether the player may drag the sprite with the mouse.
    pub draggable: bool,
    /// Sound volume in percent, which `property of` reports.
    pub volume: f64,
}

impl Default for SpriteState {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            direction: 90.0,
            size: 100.0,
            costume: 0,
            visible: true,
            rotation_style: RotationStyle::AllAround,
            draggable: false,
            volume: 100.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub name: String,
    pub is_stage: bool,
    pub num_vars: Vec<NumVar>,
    pub any_vars: Vec<AnyVar>,
    pub scripts: Vec<Script>,
    /// The costume names, in costume order. Never empty: a target with no costumes gets one unnamed
    /// costume, so that costume blocks have something to point at.
    pub costumes: Vec<String>,
    pub state: SpriteState,
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
                RtExpr::Join | RtExpr::LetterOf | RtExpr::CostumeName | RtExpr::BackdropName => Type::Str,
                RtExpr::Length
                | RtExpr::Random
                | RtExpr::Timer
                | RtExpr::SpriteX
                | RtExpr::SpriteY
                | RtExpr::SpriteDirection
                | RtExpr::SpriteSize
                | RtExpr::CostumeNumber
                | RtExpr::BackdropNumber
                | RtExpr::MouseX
                | RtExpr::MouseY => Type::Num,
                RtExpr::Contains | RtExpr::MouseDown | RtExpr::KeyPressed => Type::Bool,
                RtExpr::PropertyOf => Type::Any,
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
        let s = &t.state;
        if t.is_stage {
            out.push_str(&format!("  backdrop {} volume {}\n", s.costume, s.volume));
        } else {
            out.push_str(&format!(
                "  at ({}, {}) direction {} size {} costume {}{} {:?}{} volume {}\n",
                s.x,
                s.y,
                s.direction,
                s.size,
                s.costume,
                if s.visible { "" } else { " hidden" },
                s.rotation_style,
                if s.draggable { " draggable" } else { "" },
                s.volume
            ));
        }
        out.push_str(&format!("  costumes {:?}\n", t.costumes));
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
