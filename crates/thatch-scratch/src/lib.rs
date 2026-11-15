//! The Scratch frontend: lowers a Scratch 3 project to the Thatch IR.
//!
//! Every supported block is listed in `docs/blocks.md`; keep the two in step. Blocks that are not
//! supported yet produce a warning and do nothing (a reporter gives 0), so a project still compiles.
//!
//! Lowering runs to a fixed point over variable and parameter types. Every variable starts out
//! numeric if its initial value is, and every custom block parameter starts out numeric; each pass
//! demotes a variable to dynamically typed when something stores a non-number in it, and a parameter
//! when some call passes one. The last pass, where nothing changes, produces the program.

use std::collections::{HashMap, HashSet};

use thatch_ir::{
    AnyVar, BinOp, Const, Expr, FuncId, Function, Hat, MATH_OPS, NumVar, Param, Program, RtExpr, RtStmt, Script, Stmt,
    Target, Type, UnOp, VarRef,
};
use thatch_sb3::{Block, Input, Primitive, Project, field_id, field_value, parse_input};

/// How deeply blocks may nest, counting C blocks and reporters inside inputs. Real projects stay far
/// below this; generated ones with long arithmetic chains can reach a few hundred.
pub const MAX_NESTING: usize = 1000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the project has no stage")]
    NoStage,
    #[error("{target}: block {block} is part of a cycle of blocks, which is not a valid script")]
    Cycle { target: String, block: String },
    #[error("{target}: block {block} is nested more than {MAX_NESTING} blocks deep")]
    TooDeep { target: String, block: String },
    #[error("{target}: custom block {proccode:?} has {ids} argument ids but {names} argument names")]
    ArgumentMetadata { target: String, proccode: String, ids: usize, names: usize },
    #[error("could not start the thread the compiler runs on: {0}")]
    Thread(#[source] std::io::Error),
}

/// Something the frontend could not translate faithfully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub target: String,
    pub block: String,
    pub message: String,
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.target, self.block, self.message)
    }
}

/// The result of lowering.
#[derive(Debug)]
pub struct Lowered {
    pub program: Program,
    pub warnings: Vec<Warning>,
}

/// The stack lowering runs on. It recurses once per nesting level, and a project nested
/// [`MAX_NESTING`] deep needs more than a usual thread's stack in a debug build. Only the pages
/// actually used cost memory.
const LOWERING_STACK: usize = 256 << 20;

/// Lowers a project to the IR. The work runs on a thread of its own with a large stack, so any
/// caller can lower a project nested up to [`MAX_NESTING`] deep, whatever its own stack. When
/// that thread cannot be started, this is [`Error::Thread`] rather than lowering on the caller's
/// stack, which a deep project could overflow.
pub fn lower(project: &Project) -> Result<Lowered, Error> {
    std::thread::scope(|scope| {
        let handle = std::thread::Builder::new()
            .name("thatch-lower".to_owned())
            .stack_size(LOWERING_STACK)
            .spawn_scoped(scope, || lower_here(project))
            .map_err(Error::Thread)?;
        handle.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn lower_here(project: &Project) -> Result<Lowered, Error> {
    let mut targets: Vec<&thatch_sb3::Target> = project.targets.iter().filter(|t| !t.is_stage).collect();
    targets.sort_by_key(|t| t.layer_order);
    let stage = project.stage().ok_or(Error::NoStage)?;
    targets.insert(0, stage);

    let mut types = Types::new(&targets)?;
    loop {
        let mut lowerer = Lowerer::new(&targets, &types);
        let program = lowerer.lower_all();
        if let Some(error) = lowerer.error.take() {
            return Err(error);
        }
        if lowerer.changed {
            types = lowerer.types;
            continue;
        }
        return Ok(Lowered { program, warnings: lowerer.warnings });
    }
}

/// A variable of a target, as the fixed point sees it.
#[derive(Debug, Clone)]
struct VarInfo {
    id: String,
    name: String,
    initial: serde_json::Value,
    numeric: bool,
    /// The index among the target's numeric or dynamically typed variables, by `numeric`.
    index: u32,
}

/// A custom block of a target.
#[derive(Debug, Clone)]
struct ProcInfo {
    proccode: String,
    definition: String,
    arg_ids: Vec<String>,
    arg_names: Vec<String>,
    /// Per parameter: Bool for boolean parameters, otherwise Num until a call demotes it to Any.
    param_types: Vec<Type>,
    warp: bool,
}

/// The type state the fixed point iterates on.
#[derive(Debug, Clone)]
struct Types {
    vars: Vec<Vec<VarInfo>>,
    procs: Vec<Vec<ProcInfo>>,
}

impl Types {
    fn new(targets: &[&thatch_sb3::Target]) -> Result<Self, Error> {
        let vars = targets
            .iter()
            .map(|t| {
                let mut vars: Vec<VarInfo> = t
                    .variable_list()
                    .into_iter()
                    .map(|v| {
                        let numeric = initial_number(&v.value).is_some();
                        VarInfo { id: v.id, name: v.name, initial: v.value, numeric, index: 0 }
                    })
                    .collect();
                assign_indices(&mut vars);
                vars
            })
            .collect();
        let procs = targets.iter().map(|t| find_procedures(t)).collect::<Result<_, _>>()?;
        Ok(Self { vars, procs })
    }
}

fn assign_indices(vars: &mut [VarInfo]) {
    let (mut nums, mut anys) = (0, 0);
    for v in vars {
        if v.numeric {
            v.index = nums;
            nums += 1;
        } else {
            v.index = anys;
            anys += 1;
        }
    }
}

/// The number a variable's initial value stands for, when it is safe to keep it as a number.
fn initial_number(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) if thatch_values::is_safe_number_literal(s) => Some(thatch_values::js_number(s)),
        _ => None,
    }
}

fn find_procedures(target: &thatch_sb3::Target) -> Result<Vec<ProcInfo>, Error> {
    let mut procs = Vec::new();
    for (id, block) in target.all_blocks() {
        if block.opcode != "procedures_definition" || !block.top_level {
            continue;
        }
        let Input::Block(proto_id) = parse_input(block.inputs.get("custom_block").unwrap_or(&serde_json::Value::Null))
        else {
            continue;
        };
        let Some(proto) = target.block(&proto_id) else { continue };
        let mutation = proto.mutation.clone().unwrap_or_default();
        let arg_ids = mutation.argument_ids();
        let arg_names = mutation.argument_names();
        if arg_ids.len() != arg_names.len() {
            return Err(Error::ArgumentMetadata {
                target: target.name.clone(),
                proccode: mutation.proccode.clone(),
                ids: arg_ids.len(),
                names: arg_names.len(),
            });
        }
        let param_types = arg_ids
            .iter()
            .map(|arg_id| {
                let is_bool = match parse_input(proto.inputs.get(arg_id).unwrap_or(&serde_json::Value::Null)) {
                    Input::Block(reporter) => {
                        target.block(&reporter).is_some_and(|b| b.opcode == "argument_reporter_boolean")
                    }
                    _ => false,
                };
                if is_bool { Type::Bool } else { Type::Num }
            })
            .collect();
        procs.push(ProcInfo {
            proccode: mutation.proccode.clone(),
            definition: id,
            arg_ids,
            arg_names,
            param_types,
            warp: mutation.is_warp(),
        });
    }
    Ok(procs)
}

struct Lowerer<'a> {
    targets: &'a [&'a thatch_sb3::Target],
    types: Types,
    changed: bool,
    warnings: Vec<Warning>,
    /// The first malformed thing found; lowering goes on but the result is thrown away.
    error: Option<Error>,
    program: Program,
    /// Function ids of procedures, by (target, procedure index).
    proc_funcs: HashMap<(usize, usize), FuncId>,
    // State of the function being lowered.
    target: usize,
    proc: Option<usize>,
    nesting: u32,
    /// The blocks being lowered right now, from the script's top down to the current block, to
    /// notice a block that leads back to itself.
    visiting: HashSet<String>,
    /// How many stacks and reporters the lowering is inside, which is how deep it recurses. A
    /// long flat script counts once, however many blocks follow one another.
    depth: usize,
}

impl<'a> Lowerer<'a> {
    fn new(targets: &'a [&'a thatch_sb3::Target], types: &Types) -> Self {
        Self {
            targets,
            types: types.clone(),
            changed: false,
            warnings: Vec::new(),
            error: None,
            program: Program::default(),
            proc_funcs: HashMap::new(),
            target: 0,
            proc: None,
            nesting: 0,
            visiting: HashSet::new(),
            depth: 0,
        }
    }

    /// Records a malformed script. The first one wins.
    fn fail(&mut self, error: Error) {
        if self.error.is_none() {
            self.error = Some(error);
        }
    }

    /// Goes one level deeper, into a stack or a reporter starting at block `id`, or records that the
    /// project nests too deeply. The bound makes a deeply nested project an error and not a stack
    /// overflow. Pair it with [`Lowerer::shallower`].
    fn deeper(&mut self, id: &str) -> bool {
        if self.depth >= MAX_NESTING {
            self.fail(Error::TooDeep { target: self.targets[self.target].name.clone(), block: id.to_owned() });
            return false;
        }
        self.depth += 1;
        true
    }

    fn shallower(&mut self) {
        self.depth -= 1;
    }

    /// Marks a block as being lowered, or records a cycle when it already is.
    fn enter(&mut self, id: &str) -> bool {
        if !self.visiting.insert(id.to_owned()) {
            self.fail(Error::Cycle { target: self.targets[self.target].name.clone(), block: id.to_owned() });
            return false;
        }
        true
    }

    fn leave(&mut self, id: &str) {
        self.visiting.remove(id);
    }

    fn lower_all(&mut self) -> Program {
        // Declare every function first so calls can refer to procedures defined later.
        for (ti, target) in self.targets.iter().enumerate() {
            for (pi, proc) in self.types.procs[ti].iter().enumerate() {
                let f = Function {
                    name: format!("{}/{}", target.name, proc.proccode),
                    params: Vec::new(),
                    warp: proc.warp,
                    body: Vec::new(),
                };
                let id = self.program.add_function(f);
                self.proc_funcs.insert((ti, pi), id);
            }
        }
        for ti in 0..self.targets.len() {
            let target = self.lower_target(ti);
            self.program.targets.push(target);
        }
        std::mem::take(&mut self.program)
    }

    fn lower_target(&mut self, ti: usize) -> Target {
        let target = self.targets[ti];
        self.target = ti;
        for v in target.variable_list() {
            if v.cloud {
                self.warn(&v.id, format!("cloud variable {:?} is a plain variable here; nothing syncs it", v.name));
            }
        }
        let mut scripts = Vec::new();
        for (id, block) in target.all_blocks() {
            if !block.top_level || block.shadow {
                continue;
            }
            match block.opcode.as_str() {
                "event_whenflagclicked" => {
                    let func = self.lower_script(&block, format!("{}/flag {}", target.name, id));
                    scripts.push(Script { hat: Hat::FlagClicked, func });
                }
                "event_whenbroadcastreceived" => {
                    let name = field_value(&block, "BROADCAST_OPTION").unwrap_or_default();
                    let func = self.lower_script(&block, format!("{}/receive {}", target.name, name));
                    scripts.push(Script { hat: Hat::BroadcastReceived(name), func });
                }
                "procedures_definition" => {
                    if let Some(pi) = self.types.procs[ti].iter().position(|p| p.definition == id) {
                        self.lower_procedure(pi);
                    }
                }
                op if op.starts_with("event_when") || op == "control_start_as_clone" => {
                    self.warn(&id, format!("hat block {op} is not supported yet, so its script never runs"));
                }
                _ => {}
            }
        }
        let mut num_vars = Vec::new();
        let mut any_vars = Vec::new();
        for v in &self.types.vars[ti] {
            if v.numeric {
                num_vars.push(NumVar { name: v.name.clone(), initial: initial_number(&v.initial).unwrap_or(0.0) });
            } else {
                let initial = match &v.initial {
                    serde_json::Value::Number(n) => Const::Num(n.as_f64().unwrap_or(0.0)),
                    serde_json::Value::Bool(b) => Const::Bool(*b),
                    other => Const::Str(self.program.intern(&thatch_sb3::json_to_text(other))),
                };
                any_vars.push(AnyVar { name: v.name.clone(), initial });
            }
        }
        Target { name: target.name.clone(), is_stage: target.is_stage, num_vars, any_vars, scripts }
    }

    fn lower_script(&mut self, hat: &Block, name: String) -> FuncId {
        self.proc = None;
        self.nesting = 0;
        let body = self.lower_stack(hat.next.as_deref());
        self.program.add_function(Function { name, params: Vec::new(), warp: false, body })
    }

    fn lower_procedure(&mut self, pi: usize) {
        let ti = self.target;
        let proc = self.types.procs[ti][pi].clone();
        self.proc = Some(pi);
        self.nesting = 0;
        let definition = self.targets[ti].block(&proc.definition);
        let body = self.lower_stack(definition.as_ref().and_then(|b| b.next.as_deref()));
        let params = proc
            .arg_names
            .iter()
            .zip(&proc.param_types)
            .map(|(name, ty)| Param { name: name.clone(), ty: *ty })
            .collect();
        let func = self.proc_funcs[&(ti, pi)];
        let f = &mut self.program.functions[func.0 as usize];
        f.params = params;
        f.body = body;
    }

    fn warn(&mut self, block: &str, message: String) {
        self.warnings.push(Warning {
            target: self.targets[self.target].name.clone(),
            block: block.to_owned(),
            message,
        });
    }

    fn block(&self, id: &str) -> Option<Block> {
        self.targets[self.target].block(id)
    }

    // ---- statements -----------------------------------------------------------------------------

    fn lower_stack(&mut self, first: Option<&str>) -> Vec<Stmt> {
        let mut stmts = Vec::new();
        let Some(first) = first else { return stmts };
        if !self.deeper(first) {
            return stmts;
        }
        let mut chain = Vec::new();
        let mut next = Some(first.to_owned());
        while let Some(id) = next {
            let Some(block) = self.block(&id) else { break };
            if self.error.is_some() || !self.enter(&id) {
                break;
            }
            chain.push(id.clone());
            self.lower_block(&id, &block, &mut stmts);
            next = block.next.clone();
        }
        for id in chain {
            self.leave(&id);
        }
        self.shallower();
        stmts
    }

    fn substack(&mut self, block: &Block, name: &str) -> Vec<Stmt> {
        let first = match parse_input(block.inputs.get(name).unwrap_or(&serde_json::Value::Null)) {
            Input::Block(id) => Some(id),
            _ => None,
        };
        self.nesting += 1;
        let body = self.lower_stack(first.as_deref());
        self.nesting -= 1;
        body
    }

    /// The body of a loop: the substack and then the yield at the end of each iteration. The yield
    /// is there in warp procedures too; the runtime decides what it does, since a procedure called
    /// from a warp procedure inherits warp mode.
    fn loop_body(&mut self, block: &Block) -> Vec<Stmt> {
        let mut body = self.substack(block, "SUBSTACK");
        body.push(Stmt::Yield);
        body
    }

    fn lower_block(&mut self, id: &str, block: &Block, out: &mut Vec<Stmt>) {
        match block.opcode.as_str() {
            "looks_say" | "looks_think" => {
                let message = self.input(block, "MESSAGE", Type::Any);
                let rt = if block.opcode == "looks_say" { RtStmt::Say } else { RtStmt::Think };
                out.push(Stmt::Rt(rt, vec![message]));
            }
            "looks_sayforsecs" | "looks_thinkforsecs" => {
                let message = self.input(block, "MESSAGE", Type::Any);
                let secs = self.input(block, "SECS", Type::Num);
                let rt = if block.opcode == "looks_sayforsecs" { RtStmt::SayFor } else { RtStmt::ThinkFor };
                out.push(Stmt::Rt(rt, vec![message, secs]));
            }
            "control_wait" => {
                let secs = self.input(block, "DURATION", Type::Num);
                out.push(Stmt::Rt(RtStmt::Wait, vec![secs]));
            }
            "control_repeat" => {
                let times = self.input(block, "TIMES", Type::Num);
                let body = self.loop_body(block);
                out.push(Stmt::Repeat { times, body });
            }
            "control_forever" => {
                let body = self.loop_body(block);
                out.push(Stmt::Loop(body));
            }
            "control_if" => {
                let cond = self.input(block, "CONDITION", Type::Bool);
                let then = self.substack(block, "SUBSTACK");
                out.push(Stmt::If { cond, then, els: Vec::new() });
            }
            "control_if_else" => {
                let cond = self.input(block, "CONDITION", Type::Bool);
                let then = self.substack(block, "SUBSTACK");
                let els = self.substack(block, "SUBSTACK2");
                out.push(Stmt::If { cond, then, els });
            }
            "control_repeat_until" | "control_while" => {
                let mut cond = self.input(block, "CONDITION", Type::Bool);
                if block.opcode == "control_while" {
                    cond = Expr::Unary(UnOp::Not, Box::new(cond));
                }
                let mut body = vec![Stmt::If { cond, then: vec![Stmt::Break], els: Vec::new() }];
                body.extend(self.loop_body(block));
                out.push(Stmt::Loop(body));
            }
            "control_wait_until" => {
                let cond = self.input(block, "CONDITION", Type::Bool);
                let body = vec![Stmt::If { cond, then: vec![Stmt::Break], els: Vec::new() }, Stmt::Yield];
                out.push(Stmt::Loop(body));
            }
            "control_stop" => match field_value(block, "STOP_OPTION").unwrap_or_default().as_str() {
                "all" => out.push(Stmt::Rt(RtStmt::StopAll, Vec::new())),
                "other scripts in sprite" | "other scripts in stage" => {
                    out.push(Stmt::Rt(RtStmt::StopOthers, Vec::new()));
                }
                _ => out.push(Stmt::Return),
            },
            "event_broadcast" | "event_broadcastandwait" => {
                let name = self.broadcast_input(block);
                let rt = if block.opcode == "event_broadcast" { RtStmt::Broadcast } else { RtStmt::BroadcastAndWait };
                out.push(Stmt::Rt(rt, vec![name]));
            }
            "data_setvariableto" => {
                let var = self.variable(block);
                let value = self.input_raw(block, "VALUE", Type::Str);
                let value = self.store_value(var, value);
                out.push(Stmt::Set(var, value));
            }
            "data_changevariableby" => {
                let var = self.variable(block);
                let delta = self.input(block, "VALUE", Type::Num);
                let current = self.cast_to_number(Expr::Var(var));
                out.push(Stmt::Set(var, Expr::Binary(BinOp::Add, Box::new(current), Box::new(delta))));
            }
            "procedures_call" => self.lower_call(id, block, out),
            "sensing_resettimer" => out.push(Stmt::Rt(RtStmt::ResetTimer, Vec::new())),
            op => self.warn(id, format!("block {op} is not supported yet; it does nothing")),
        }
    }

    /// Fits a value to the variable it is stored in, demoting a numeric variable that receives a
    /// non-number.
    fn store_value(&mut self, var: VarRef, value: Expr) -> Expr {
        match var {
            VarRef::TargetNum(_) | VarRef::StageNum(_) => {
                if self.ty(&value) == Type::Num {
                    value
                } else {
                    self.demote_var(var);
                    value
                }
            }
            _ => value,
        }
    }

    fn demote_var(&mut self, var: VarRef) {
        let (ti, index) = match var {
            VarRef::TargetNum(i) => (self.target, i),
            VarRef::StageNum(i) => (0, i),
            _ => return,
        };
        let vars = &mut self.types.vars[ti];
        if let Some(v) = vars.iter_mut().find(|v| v.numeric && v.index == index) {
            v.numeric = false;
            self.changed = true;
            assign_indices(vars);
        }
    }

    fn lower_call(&mut self, id: &str, block: &Block, out: &mut Vec<Stmt>) {
        let mutation = block.mutation.clone().unwrap_or_default();
        let ti = self.target;
        let Some(pi) = self.types.procs[ti].iter().position(|p| p.proccode == mutation.proccode) else {
            self.warn(id, format!("custom block {:?} has no definition", mutation.proccode));
            return;
        };
        let proc = self.types.procs[ti][pi].clone();
        let mut args = Vec::new();
        for (ai, arg_id) in proc.arg_ids.iter().enumerate() {
            let want = proc.param_types[ai];
            let raw = block.inputs.get(arg_id);
            let arg = match want {
                Type::Bool => self.input(block, arg_id, Type::Bool),
                _ => match raw {
                    Some(_) => self.input_raw(block, arg_id, Type::Str),
                    None => Expr::Str(self.program.intern("")),
                },
            };
            let arg = if want == Type::Num && self.ty(&arg) != Type::Num {
                self.types.procs[ti][pi].param_types[ai] = Type::Any;
                self.changed = true;
                arg
            } else {
                self.cast(arg, want)
            };
            args.push(arg);
        }
        let func = self.proc_funcs[&(ti, pi)];
        out.push(Stmt::Call { func, args, nesting: self.nesting });
    }

    // ---- expressions ----------------------------------------------------------------------------

    /// The static type of an expression in the current function.
    fn ty(&self, e: &Expr) -> Type {
        e.ty(&|v| self.var_type(v))
    }

    fn var_type(&self, v: VarRef) -> Type {
        match v {
            VarRef::TargetNum(_) | VarRef::StageNum(_) => Type::Num,
            VarRef::TargetAny(_) | VarRef::StageAny(_) => Type::Any,
            VarRef::Param(i) => self
                .proc
                .and_then(|pi| self.types.procs[self.target][pi].param_types.get(i as usize).copied())
                .unwrap_or(Type::Any),
        }
    }

    /// Converts an expression to a type, folding literals.
    fn cast(&mut self, e: Expr, to: Type) -> Expr {
        let from = self.ty(&e);
        if from == to {
            return e;
        }
        match (&e, to) {
            (Expr::Num(n), Type::Str) => Expr::Str(self.program.intern(&thatch_values::number_to_string(*n))),
            (Expr::Num(n), Type::Bool) => Expr::Bool(thatch_values::num_to_boolean(*n)),
            (Expr::Str(s), Type::Num) => Expr::Num(thatch_values::str_to_number(&self.program.strings[s.0 as usize])),
            (Expr::Str(s), Type::Bool) => {
                Expr::Bool(thatch_values::str_to_boolean(&self.program.strings[s.0 as usize]))
            }
            (Expr::Bool(b), Type::Num) => Expr::Num(f64::from(*b)),
            (Expr::Bool(b), Type::Str) => Expr::Str(self.program.intern(thatch_values::bool_to_string(*b))),
            _ => Expr::Cast(to, Box::new(e)),
        }
    }

    /// `Cast.toNumber`: converts to a number, and turns a NaN that an earlier operation produced
    /// into 0, as every numeric block input does in Scratch.
    fn cast_to_number(&mut self, e: Expr) -> Expr {
        if self.ty(&e) != Type::Num {
            // The other conversions never give NaN.
            return self.cast(e, Type::Num);
        }
        match e {
            Expr::Num(n) => Expr::Num(thatch_values::num_to_number(n)),
            e if may_be_nan(&e) => Expr::Unary(UnOp::ToNumber, Box::new(e)),
            e => e,
        }
    }

    /// Lowers an input and converts it to `want`.
    fn input(&mut self, block: &Block, name: &str, want: Type) -> Expr {
        let e = self.input_raw(block, name, want);
        if want == Type::Num { self.cast_to_number(e) } else { self.cast(e, want) }
    }

    /// Lowers an input, keeping its own type. An empty slot gives the empty value of `default`.
    fn input_raw(&mut self, block: &Block, name: &str, default: Type) -> Expr {
        let raw = block.inputs.get(name).cloned().unwrap_or(serde_json::Value::Null);
        if !raw.is_null() && !raw.is_array() {
            self.warn(name, format!("input {name} is not an input array; it counts as empty"));
        }
        match parse_input(&raw) {
            Input::Empty => match default {
                Type::Bool => Expr::Bool(false),
                Type::Num => Expr::Num(0.0),
                _ => Expr::Str(self.program.intern("")),
            },
            Input::Primitive(p) => self.primitive(&p),
            Input::Block(id) => self.reporter(&id),
        }
    }

    fn primitive(&mut self, p: &Primitive) -> Expr {
        match p {
            Primitive::Text(s) => {
                if thatch_values::is_safe_number_literal(s) {
                    Expr::Num(thatch_values::js_number(s))
                } else {
                    Expr::Str(self.program.intern(s))
                }
            }
            Primitive::Broadcast { name, .. } => Expr::Str(self.program.intern(name)),
            Primitive::Variable { name, id } => Expr::Var(self.resolve_variable(id, name)),
            Primitive::List { .. } => {
                self.warn("list", "lists are not supported yet".to_owned());
                Expr::Str(self.program.intern(""))
            }
        }
    }

    fn reporter(&mut self, id: &str) -> Expr {
        let Some(block) = self.block(id) else {
            // A top-level primitive stored as an array, such as a variable reporter.
            if let Some(raw) = self.targets[self.target].blocks.get(id)
                && let Some(p) = thatch_sb3::parse_primitive(raw)
            {
                return self.primitive(&p);
            }
            self.warn(id, "an input refers to a block that does not exist; it reports 0".to_owned());
            return Expr::Num(0.0);
        };
        if self.error.is_some() || !self.deeper(id) {
            return Expr::Num(0.0);
        }
        if !self.enter(id) {
            self.shallower();
            return Expr::Num(0.0);
        }
        let e = self.reporter_block(id, block);
        self.leave(id);
        self.shallower();
        e
    }

    fn reporter_block(&mut self, id: &str, block: Block) -> Expr {
        let num = |this: &mut Self, name: &str| this.input(&block, name, Type::Num);
        match block.opcode.as_str() {
            "operator_add" | "operator_subtract" | "operator_multiply" | "operator_divide" | "operator_mod" => {
                let op = match block.opcode.as_str() {
                    "operator_add" => BinOp::Add,
                    "operator_subtract" => BinOp::Sub,
                    "operator_multiply" => BinOp::Mul,
                    "operator_divide" => BinOp::Div,
                    _ => BinOp::Mod,
                };
                let a = num(self, "NUM1");
                let b = num(self, "NUM2");
                Expr::Binary(op, Box::new(a), Box::new(b))
            }
            "operator_round" => Expr::Unary(UnOp::Round, Box::new(num(self, "NUM"))),
            "operator_lt" | "operator_gt" | "operator_equals" => {
                let op = match block.opcode.as_str() {
                    "operator_lt" => BinOp::Lt,
                    "operator_gt" => BinOp::Gt,
                    _ => BinOp::Eq,
                };
                let a = self.input_raw(&block, "OPERAND1", Type::Str);
                let b = self.input_raw(&block, "OPERAND2", Type::Str);
                Expr::Binary(op, Box::new(self.compare_operand(a)), Box::new(self.compare_operand(b)))
            }
            "operator_and" | "operator_or" => {
                let op = if block.opcode == "operator_and" { BinOp::And } else { BinOp::Or };
                let a = self.input(&block, "OPERAND1", Type::Bool);
                let b = self.input(&block, "OPERAND2", Type::Bool);
                Expr::Binary(op, Box::new(a), Box::new(b))
            }
            "operator_not" => {
                let a = self.input(&block, "OPERAND", Type::Bool);
                Expr::Unary(UnOp::Not, Box::new(a))
            }
            "operator_join" => {
                let a = self.input(&block, "STRING1", Type::Str);
                let b = self.input(&block, "STRING2", Type::Str);
                Expr::Rt(RtExpr::Join, vec![a, b])
            }
            "operator_letter_of" => {
                let i = num(self, "LETTER");
                let s = self.input(&block, "STRING", Type::Str);
                Expr::Rt(RtExpr::LetterOf, vec![i, s])
            }
            "operator_length" => {
                let s = self.input(&block, "STRING", Type::Str);
                Expr::Rt(RtExpr::Length, vec![s])
            }
            "operator_contains" => {
                let a = self.input(&block, "STRING1", Type::Str);
                let b = self.input(&block, "STRING2", Type::Str);
                Expr::Rt(RtExpr::Contains, vec![a, b])
            }
            "operator_random" => {
                let a = self.input_raw(&block, "FROM", Type::Num);
                let b = self.input_raw(&block, "TO", Type::Num);
                Expr::Rt(RtExpr::Random, vec![a, b])
            }
            "operator_mathop" => {
                let n = num(self, "NUM");
                let op = field_value(&block, "OPERATOR").unwrap_or_default();
                let un = match op.as_str() {
                    "abs" => UnOp::Abs,
                    "floor" => UnOp::Floor,
                    "ceiling" => UnOp::Ceil,
                    "sqrt" => UnOp::Sqrt,
                    other => match MATH_OPS.iter().position(|m| *m == other) {
                        Some(i) => UnOp::MathOp(i as u8),
                        None => {
                            self.warn(id, format!("unknown mathop {other:?}"));
                            return Expr::Num(0.0);
                        }
                    },
                };
                Expr::Unary(un, Box::new(n))
            }
            "data_variable" => {
                let name = field_value(&block, "VARIABLE").unwrap_or_default();
                let var_id = field_id(&block, "VARIABLE").unwrap_or_default();
                Expr::Var(self.resolve_variable(&var_id, &name))
            }
            "argument_reporter_string_number" | "argument_reporter_boolean" => {
                let name = field_value(&block, "VALUE").unwrap_or_default();
                let param = self.proc.and_then(|pi| {
                    let proc = &self.types.procs[self.target][pi];
                    proc.arg_names.iter().rposition(|n| *n == name)
                });
                match param {
                    Some(i) => Expr::Var(VarRef::Param(i as u32)),
                    None if block.opcode == "argument_reporter_boolean" => Expr::Bool(false),
                    None => Expr::Num(0.0),
                }
            }
            "sensing_timer" => Expr::Rt(RtExpr::Timer, Vec::new()),
            "event_broadcast_menu" => {
                let name = field_value(&block, "BROADCAST_OPTION").unwrap_or_default();
                Expr::Str(self.program.intern(&name))
            }
            op => {
                self.warn(id, format!("reporter {op} is not supported yet; it reports 0"));
                Expr::Num(0.0)
            }
        }
    }

    /// Comparison operands keep their own type so the backend can compare numbers directly. A
    /// boolean compares as the number 1 or 0 when the other side is numeric and as `"true"` or
    /// `"false"` otherwise, so it stays dynamic and the runtime decides.
    fn compare_operand(&mut self, e: Expr) -> Expr {
        if self.ty(&e) == Type::Bool { self.cast(e, Type::Any) } else { e }
    }

    fn broadcast_input(&mut self, block: &Block) -> Expr {
        self.input(block, "BROADCAST_INPUT", Type::Str)
    }

    fn variable(&mut self, block: &Block) -> VarRef {
        let name = field_value(block, "VARIABLE").unwrap_or_default();
        let id = field_id(block, "VARIABLE").unwrap_or_default();
        self.resolve_variable(&id, &name)
    }

    /// Finds a variable by id, then by name, on the current target and then the stage, creating it
    /// on the current target when it does not exist, as scratch-vm does.
    fn resolve_variable(&mut self, id: &str, name: &str) -> VarRef {
        let ti = self.target;
        for (t, by_id) in [(ti, true), (0, true), (ti, false), (0, false)] {
            let found = self.types.vars[t].iter().find(|v| if by_id { v.id == id } else { v.name == name });
            if let Some(v) = found {
                return var_ref(t == 0 && ti != 0, v);
            }
        }
        let mut info = VarInfo {
            id: id.to_owned(),
            name: name.to_owned(),
            initial: serde_json::Value::Number(0.into()),
            numeric: true,
            index: 0,
        };
        let vars = &mut self.types.vars[ti];
        info.index = vars.iter().filter(|v| v.numeric).count() as u32;
        vars.push(info);
        self.changed = true;
        let v = vars.last().expect("just pushed");
        var_ref(false, v)
    }
}

/// Whether a numeric expression can evaluate to NaN. Conversions from other types never do, and
/// neither do lengths, the timer or a rounding of something that cannot; arithmetic, variables and
/// parameters can.
fn may_be_nan(e: &Expr) -> bool {
    match e {
        Expr::Num(n) => n.is_nan(),
        Expr::Cast(Type::Num, _) => false,
        Expr::Unary(UnOp::ToNumber, _) => false,
        Expr::Unary(UnOp::Round | UnOp::Abs | UnOp::Floor | UnOp::Ceil, inner) => may_be_nan(inner),
        Expr::Rt(RtExpr::Length | RtExpr::Timer, _) => false,
        _ => true,
    }
}

fn var_ref(on_stage_from_sprite: bool, v: &VarInfo) -> VarRef {
    match (on_stage_from_sprite, v.numeric) {
        (true, true) => VarRef::StageNum(v.index),
        (true, false) => VarRef::StageAny(v.index),
        (false, true) => VarRef::TargetNum(v.index),
        (false, false) => VarRef::TargetAny(v.index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(blocks: &str, variables: &str) -> Project {
        let json = format!(
            r#"{{"targets":[{{"isStage":true,"name":"Stage","variables":{variables},"blocks":{{}}}},
            {{"isStage":false,"name":"Cat","variables":{{}},"blocks":{blocks},"layerOrder":1}}]}}"#
        );
        Project::from_json(&json).unwrap()
    }

    #[test]
    fn numeric_variables_stay_numeric_until_a_string_is_stored() {
        let p = project(
            r#"{"h":{"opcode":"event_whenflagclicked","next":"s","parent":null,"inputs":{},"fields":{},"topLevel":true},
            "s":{"opcode":"data_setvariableto","next":"t","parent":"h","inputs":{"VALUE":[1,[10,"5"]]},"fields":{"VARIABLE":["a","va"]},"topLevel":false},
            "t":{"opcode":"data_setvariableto","next":null,"parent":"s","inputs":{"VALUE":[1,[10,"hi"]]},"fields":{"VARIABLE":["b","vb"]},"topLevel":false}}"#,
            r#"{"va":["a",0],"vb":["b",0]}"#,
        );
        let lowered = lower(&p).unwrap();
        let stage = &lowered.program.targets[0];
        assert_eq!(stage.num_vars.len(), 1, "{}", thatch_ir::dump(&lowered.program));
        assert_eq!(stage.num_vars[0].name, "a");
        assert_eq!(stage.any_vars.len(), 1);
        assert_eq!(stage.any_vars[0].name, "b");
        let body = &lowered.program.functions[0].body;
        assert_eq!(body[0], Stmt::Set(VarRef::StageNum(0), Expr::Num(5.0)));
        assert!(matches!(body[1], Stmt::Set(VarRef::StageAny(0), Expr::Str(_))));
        assert!(lowered.warnings.is_empty());
    }

    #[test]
    fn unsupported_blocks_warn() {
        let p = project(
            r#"{"h":{"opcode":"event_whenflagclicked","next":"m","parent":null,"inputs":{},"fields":{},"topLevel":true},
            "m":{"opcode":"motion_movesteps","next":null,"parent":"h","inputs":{"STEPS":[1,[4,"10"]]},"fields":{},"topLevel":false}}"#,
            "{}",
        );
        let lowered = lower(&p).unwrap();
        assert_eq!(lowered.warnings.len(), 1);
        assert!(lowered.warnings[0].message.contains("motion_movesteps"));
    }

    /// A chain of `+` reporters nested `depth` deep under a `say`.
    fn nested_adds(depth: usize) -> Project {
        let mut blocks = vec![
            r#""h":{"opcode":"event_whenflagclicked","next":"s","parent":null,"inputs":{},"fields":{},"topLevel":true}"#
                .to_owned(),
            r#""s":{"opcode":"looks_say","next":null,"parent":"h","inputs":{"MESSAGE":[3,"a0",[10,""]]},"fields":{},"topLevel":false}"#
                .to_owned(),
        ];
        for i in 0..depth {
            let num1 = if i + 1 < depth { format!(r#"[3,"a{}",[4,""]]"#, i + 1) } else { r#"[1,[4,"1"]]"#.to_owned() };
            blocks.push(format!(
                r#""a{i}":{{"opcode":"operator_add","next":null,"parent":"s","inputs":{{"NUM1":{num1},"NUM2":[1,[4,"1"]]}},"fields":{{}},"topLevel":false}}"#
            ));
        }
        project(&format!("{{{}}}", blocks.join(",")), "{}")
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        // This runs on a test thread's small stack: `lower` gives itself a large one. The script's
        // stack is one level and the `+` chain the rest.
        assert!(lower(&nested_adds(MAX_NESTING - 1)).is_ok());
        let err = lower(&nested_adds(MAX_NESTING)).unwrap_err();
        assert!(matches!(err, Error::TooDeep { .. }), "{err}");
    }

    #[test]
    fn long_flat_scripts_are_not_deep() {
        let mut blocks = vec![
            r#""h":{"opcode":"event_whenflagclicked","next":"s0","parent":null,"inputs":{},"fields":{},"topLevel":true}"#
                .to_owned(),
        ];
        let count = MAX_NESTING * 5;
        for i in 0..count {
            let next = if i + 1 < count { format!(r#""s{}""#, i + 1) } else { "null".to_owned() };
            blocks.push(format!(
                r#""s{i}":{{"opcode":"looks_say","next":{next},"parent":"h","inputs":{{"MESSAGE":[1,[10,"hi"]]}},"fields":{{}},"topLevel":false}}"#
            ));
        }
        let lowered = lower(&project(&format!("{{{}}}", blocks.join(",")), "{}")).unwrap();
        assert_eq!(lowered.program.functions[0].body.len(), count);
    }
}
