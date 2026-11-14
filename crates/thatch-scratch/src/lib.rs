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
    AnyVar, BinOp, Const, Effect, Expr, FuncId, Function, Hat, MATH_OPS, NumVar, Param, Program, RotationStyle, RtExpr,
    RtStmt, Script, SpriteState, Stmt, Target, Type, UnOp, VarRef,
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
                "event_whenkeypressed" => {
                    let key = field_value(&block, "KEY_OPTION").unwrap_or_default();
                    let func = self.lower_script(&block, format!("{}/key {}", target.name, key));
                    scripts.push(Script { hat: Hat::KeyPressed(key), func });
                }
                "event_whenthisspriteclicked" | "event_whenstageclicked" => {
                    let func = self.lower_script(&block, format!("{}/clicked {}", target.name, id));
                    let hat = if block.opcode == "event_whenthisspriteclicked" {
                        Hat::SpriteClicked
                    } else {
                        Hat::StageClicked
                    };
                    scripts.push(Script { hat, func });
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
        let mut costumes = target.costume_names();
        if costumes.is_empty() {
            costumes.push(String::new());
        }
        // The loader clamps the costume index and wraps the direction, as scratch-vm's does.
        let costume = target.current_costume.clamp(0, costumes.len() as i64 - 1) as u32;
        let rotation_style = match target.rotation_style.as_str() {
            "left-right" => RotationStyle::LeftRight,
            "don't rotate" => RotationStyle::DontRotate,
            _ => RotationStyle::AllAround,
        };
        let state = SpriteState {
            x: target.x,
            y: target.y,
            direction: thatch_values::wrap_direction(target.direction),
            size: target.size,
            costume,
            visible: target.visible,
            rotation_style,
            draggable: target.draggable,
            volume: target.volume,
        };
        Target { name: target.name.clone(), is_stage: target.is_stage, num_vars, any_vars, scripts, costumes, state }
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
                let value = match var {
                    VarRef::TargetAny(_) | VarRef::StageAny(_) => self.input_dynamic(block, "VALUE"),
                    _ => self.input_raw(block, "VALUE", Type::Str),
                };
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

            // Motion.
            "motion_movesteps" => {
                let steps = self.input(block, "STEPS", Type::Num);
                out.push(Stmt::Rt(RtStmt::MoveSteps, vec![steps]));
            }
            "motion_gotoxy" => {
                let x = self.input(block, "X", Type::Num);
                let y = self.input(block, "Y", Type::Num);
                out.push(Stmt::Rt(RtStmt::GoToXY, vec![x, y]));
            }
            "motion_goto" => {
                let to = self.input(block, "TO", Type::Str);
                out.push(Stmt::Rt(RtStmt::GoTo, vec![to]));
            }
            "motion_turnright" | "motion_turnleft" => {
                let mut degrees = self.input(block, "DEGREES", Type::Num);
                if block.opcode == "motion_turnleft" {
                    degrees = Expr::Binary(BinOp::Sub, Box::new(Expr::Num(0.0)), Box::new(degrees));
                }
                out.push(Stmt::Rt(RtStmt::Turn, vec![degrees]));
            }
            "motion_pointindirection" => {
                let direction = self.input(block, "DIRECTION", Type::Num);
                out.push(Stmt::Rt(RtStmt::PointInDirection, vec![direction]));
            }
            "motion_pointtowards" => {
                let towards = self.input(block, "TOWARDS", Type::Str);
                out.push(Stmt::Rt(RtStmt::PointTowards, vec![towards]));
            }
            "motion_glidesecstoxy" => {
                let secs = self.input(block, "SECS", Type::Num);
                let x = self.input(block, "X", Type::Num);
                let y = self.input(block, "Y", Type::Num);
                out.push(Stmt::Rt(RtStmt::Glide, vec![secs, x, y]));
            }
            "motion_glideto" => {
                let secs = self.input(block, "SECS", Type::Num);
                let to = self.input(block, "TO", Type::Str);
                out.push(Stmt::Rt(RtStmt::GlideTo, vec![secs, to]));
            }
            "motion_ifonedgebounce" => out.push(Stmt::Rt(RtStmt::Bounce, Vec::new())),
            "motion_setrotationstyle" => {
                let style = match field_value(block, "STYLE").unwrap_or_default().as_str() {
                    "all around" => RotationStyle::AllAround,
                    "left-right" => RotationStyle::LeftRight,
                    "don't rotate" => RotationStyle::DontRotate,
                    // scratch-vm leaves the style alone for anything else.
                    _ => return,
                };
                out.push(Stmt::Rt(RtStmt::SetRotationStyle(style), Vec::new()));
            }
            "motion_changexby" | "motion_setx" | "motion_changeyby" | "motion_sety" => {
                let (rt, name) = match block.opcode.as_str() {
                    "motion_changexby" => (RtStmt::ChangeX, "DX"),
                    "motion_setx" => (RtStmt::SetX, "X"),
                    "motion_changeyby" => (RtStmt::ChangeY, "DY"),
                    _ => (RtStmt::SetY, "Y"),
                };
                let v = self.input(block, name, Type::Num);
                out.push(Stmt::Rt(rt, vec![v]));
            }
            // Scratch 1.4 blocks that scratch-vm keeps as no-ops.
            "motion_scroll_right"
            | "motion_scroll_up"
            | "motion_align_scene"
            | "looks_hideallsprites"
            | "looks_changestretchby"
            | "looks_setstretchto" => {}

            // Looks.
            "looks_show" => out.push(Stmt::Rt(RtStmt::Show, Vec::new())),
            "looks_hide" => out.push(Stmt::Rt(RtStmt::Hide, Vec::new())),
            "looks_switchcostumeto" => {
                let costume = self.input_text(block, "COSTUME");
                let costume = self.cast(costume, Type::Any);
                out.push(Stmt::Rt(RtStmt::SwitchCostume, vec![costume]));
            }
            "looks_nextcostume" => out.push(Stmt::Rt(RtStmt::NextCostume, Vec::new())),
            "looks_switchbackdropto" | "looks_switchbackdroptoandwait" => {
                // Nothing waits yet: the hats a backdrop switch starts are not supported, so
                // "and wait" has nothing to wait for.
                let backdrop = self.input_text(block, "BACKDROP");
                let backdrop = self.cast(backdrop, Type::Any);
                out.push(Stmt::Rt(RtStmt::SwitchBackdrop, vec![backdrop]));
            }
            "looks_nextbackdrop" => out.push(Stmt::Rt(RtStmt::NextBackdrop, Vec::new())),
            "looks_changesizeby" => {
                let change = self.input(block, "CHANGE", Type::Num);
                out.push(Stmt::Rt(RtStmt::ChangeSize, vec![change]));
            }
            "looks_setsizeto" => {
                let size = self.input(block, "SIZE", Type::Num);
                out.push(Stmt::Rt(RtStmt::SetSize, vec![size]));
            }
            "looks_changeeffectby" | "looks_seteffectto" => {
                let name = field_value(block, "EFFECT").unwrap_or_default().to_lowercase();
                let (input, make): (&str, fn(Effect) -> RtStmt) = if block.opcode == "looks_seteffectto" {
                    ("VALUE", RtStmt::SetEffect)
                } else {
                    ("CHANGE", RtStmt::ChangeEffect)
                };
                let value = self.input(block, input, Type::Num);
                // An unknown effect does nothing in scratch-vm.
                if let Some(effect) = effect_by_name(&name) {
                    out.push(Stmt::Rt(make(effect), vec![value]));
                }
            }
            "looks_cleargraphiceffects" => out.push(Stmt::Rt(RtStmt::ClearEffects, Vec::new())),
            "looks_gotofrontback" => {
                let front = field_value(block, "FRONT_BACK").unwrap_or_default() == "front";
                out.push(Stmt::Rt(RtStmt::GoToLayer(front), Vec::new()));
            }
            "looks_goforwardbackwardlayers" => {
                let mut n = self.input(block, "NUM", Type::Num);
                if field_value(block, "FORWARD_BACKWARD").unwrap_or_default() != "forward" {
                    n = Expr::Binary(BinOp::Sub, Box::new(Expr::Num(0.0)), Box::new(n));
                }
                out.push(Stmt::Rt(RtStmt::GoLayers, vec![n]));
            }
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
                    Some(_) if want == Type::Num => self.input_raw(block, arg_id, Type::Str),
                    Some(_) => self.input_dynamic(block, arg_id),
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

    /// An input stored in a dynamically typed variable or parameter. A typed value stays text, as
    /// it is in scratch-vm, since the slot keeps it for blocks that tell text from numbers (see
    /// [`Lowerer::input_text`]); a numeric slot gets the number, which is the same to every block
    /// that may read it.
    fn input_dynamic(&mut self, block: &Block, name: &str) -> Expr {
        let raw = block.inputs.get(name).cloned().unwrap_or(serde_json::Value::Null);
        if let Input::Primitive(Primitive::Text(s)) = parse_input(&raw) {
            return Expr::Str(self.program.intern(&s));
        }
        self.input_raw(block, name, Type::Str)
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
            // A number in the file is a number to scratch-vm too.
            Primitive::Number(n) => Expr::Num(*n),
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
            "motion_xposition" => Expr::Rt(RtExpr::SpriteX, Vec::new()),
            "motion_yposition" => Expr::Rt(RtExpr::SpriteY, Vec::new()),
            "motion_direction" => Expr::Rt(RtExpr::SpriteDirection, Vec::new()),
            "looks_size" => Expr::Rt(RtExpr::SpriteSize, Vec::new()),
            "looks_costumenumbername" | "looks_backdropnumbername" => {
                let number = field_value(&block, "NUMBER_NAME").unwrap_or_default() == "number";
                let rt = match (block.opcode == "looks_costumenumbername", number) {
                    (true, true) => RtExpr::CostumeNumber,
                    (true, false) => RtExpr::CostumeName,
                    (false, true) => RtExpr::BackdropNumber,
                    (false, false) => RtExpr::BackdropName,
                };
                Expr::Rt(rt, Vec::new())
            }
            "sensing_of" => {
                let property = field_value(&block, "PROPERTY").unwrap_or_default();
                let property = Expr::Str(self.program.intern(&property));
                let object = self.input(&block, "OBJECT", Type::Str);
                Expr::Rt(RtExpr::PropertyOf, vec![property, object])
            }
            // Menus: their one field is the value.
            "motion_goto_menu" | "motion_glideto_menu" => self.menu(&block, "TO"),
            "motion_pointtowards_menu" => self.menu(&block, "TOWARDS"),
            "looks_costume" => self.menu(&block, "COSTUME"),
            "looks_backdrops" => self.menu(&block, "BACKDROP"),
            "sensing_of_object_menu" => self.menu(&block, "OBJECT"),
            "sensing_keyoptions" => self.menu(&block, "KEY_OPTION"),
            "sensing_mousex" => Expr::Rt(RtExpr::MouseX, Vec::new()),
            "sensing_mousey" => Expr::Rt(RtExpr::MouseY, Vec::new()),
            "sensing_mousedown" => Expr::Rt(RtExpr::MouseDown, Vec::new()),
            "sensing_keypressed" => {
                let key = self.input_text(&block, "KEY_OPTION");
                let key = self.cast(key, Type::Any);
                Expr::Rt(RtExpr::KeyPressed, vec![key])
            }
            // Legacy reporters scratch-vm keeps as no-ops that report nothing.
            "motion_xscroll" | "motion_yscroll" => Expr::Num(0.0),
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

    /// Lowers an input whose block asks whether its value is a number: `key pressed?` and the
    /// costume and backdrop blocks treat a number and the same digits as text differently. A value
    /// typed into the project is text to scratch-vm unless the file stores a JSON number, so it
    /// stays as the file has it. A variable or parameter read here is never specialized to a
    /// number, since a numeric slot could not tell the text "2" from the number 2.
    fn input_text(&mut self, block: &Block, name: &str) -> Expr {
        let raw = block.inputs.get(name).cloned().unwrap_or(serde_json::Value::Null);
        if let Input::Primitive(Primitive::Text(s)) = parse_input(&raw) {
            return Expr::Str(self.program.intern(&s));
        }
        let e = self.input_raw(block, name, Type::Str);
        match e {
            Expr::Var(var @ (VarRef::TargetNum(_) | VarRef::StageNum(_))) => self.demote_var(var),
            Expr::Var(VarRef::Param(i)) => {
                if let Some(pi) = self.proc {
                    let ty = &mut self.types.procs[self.target][pi].param_types[i as usize];
                    if *ty == Type::Num {
                        *ty = Type::Any;
                        self.changed = true;
                    }
                }
            }
            _ => {}
        }
        e
    }

    /// The value of a menu block's field, as a string.
    fn menu(&mut self, block: &Block, field: &str) -> Expr {
        let value = field_value(block, field).unwrap_or_default();
        Expr::Str(self.program.intern(&value))
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
        Expr::Rt(RtExpr::Length | RtExpr::Timer | RtExpr::MouseX | RtExpr::MouseY, _) => false,
        _ => true,
    }
}

/// The graphic effect a lowercased `EFFECT` field names.
fn effect_by_name(name: &str) -> Option<Effect> {
    Some(match name {
        "color" => Effect::Color,
        "fisheye" => Effect::Fisheye,
        "whirl" => Effect::Whirl,
        "pixelate" => Effect::Pixelate,
        "mosaic" => Effect::Mosaic,
        "brightness" => Effect::Brightness,
        "ghost" => Effect::Ghost,
        _ => return None,
    })
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
            "m":{"opcode":"pen_down","next":null,"parent":"h","inputs":{},"fields":{},"topLevel":false}}"#,
            "{}",
        );
        let lowered = lower(&p).unwrap();
        assert_eq!(lowered.warnings.len(), 1);
        assert!(lowered.warnings[0].message.contains("pen_down"));
    }

    #[test]
    fn sprite_state_comes_from_the_project() {
        let json = r#"{"targets":[{"isStage":true,"name":"Stage","costumes":[{"name":"b1"},{"name":"b2"}],"currentCostume":5},
            {"isStage":false,"name":"Cat","blocks":{},"layerOrder":1,"x":1,"y":2,"direction":270,"size":50,
             "visible":false,"rotationStyle":"left-right","draggable":true,"volume":40,"costumes":[{"name":"a"}]}]}"#;
        let p = Project::from_json(json).unwrap();
        let lowered = lower(&p).unwrap();
        let stage = &lowered.program.targets[0];
        assert_eq!(stage.costumes, ["b1", "b2"]);
        assert_eq!(stage.state.costume, 1, "the costume index is clamped");
        let cat = &lowered.program.targets[1];
        assert_eq!(cat.costumes, ["a"]);
        let s = &cat.state;
        assert_eq!((s.x, s.y, s.direction, s.size, s.volume), (1.0, 2.0, -90.0, 50.0, 40.0));
        assert!(!s.visible);
        assert!(s.draggable);
        assert_eq!(s.rotation_style, RotationStyle::LeftRight);
    }

    #[test]
    fn motion_and_looks_blocks_lower() {
        let p = project(
            r#"{"h":{"opcode":"event_whenflagclicked","next":"m","parent":null,"inputs":{},"fields":{},"topLevel":true},
            "m":{"opcode":"motion_turnleft","next":"g","parent":"h","inputs":{"DEGREES":[1,[4,"15"]]},"fields":{},"topLevel":false},
            "g":{"opcode":"motion_goto","next":"c","parent":"m","inputs":{"TO":[1,"gm"]},"fields":{},"topLevel":false},
            "gm":{"opcode":"motion_goto_menu","next":null,"parent":"g","inputs":{},"fields":{"TO":["_random_",null]},"shadow":true,"topLevel":false},
            "c":{"opcode":"looks_switchcostumeto","next":"e","parent":"g","inputs":{"COSTUME":[1,"cm"]},"fields":{},"topLevel":false},
            "cm":{"opcode":"looks_costume","next":null,"parent":"c","inputs":{},"fields":{"COSTUME":["costume2",null]},"shadow":true,"topLevel":false},
            "e":{"opcode":"looks_seteffectto","next":"s","parent":"c","inputs":{"VALUE":[1,[4,"25"]]},"fields":{"EFFECT":["GHOST",null]},"topLevel":false},
            "s":{"opcode":"looks_say","next":null,"parent":"e","inputs":{"MESSAGE":[3,"of",[10,""]]},"fields":{},"topLevel":false},
            "of":{"opcode":"sensing_of","next":null,"parent":"s","inputs":{"OBJECT":[1,"om"]},"fields":{"PROPERTY":["x position",null]},"topLevel":false},
            "om":{"opcode":"sensing_of_object_menu","next":null,"parent":"of","inputs":{},"fields":{"OBJECT":["Cat",null]},"shadow":true,"topLevel":false}}"#,
            "{}",
        );
        let lowered = lower(&p).unwrap();
        assert!(lowered.warnings.is_empty(), "{:?}", lowered.warnings);
        let body = &lowered.program.functions[0].body;
        assert_eq!(
            body[0],
            Stmt::Rt(RtStmt::Turn, vec![Expr::Binary(BinOp::Sub, Box::new(Expr::Num(0.0)), Box::new(Expr::Num(15.0)))])
        );
        assert!(matches!(&body[1], Stmt::Rt(RtStmt::GoTo, args) if matches!(args[0], Expr::Str(_))));
        assert!(
            matches!(&body[2], Stmt::Rt(RtStmt::SwitchCostume, args) if matches!(args[0], Expr::Cast(Type::Any, _)))
        );
        assert_eq!(body[3], Stmt::Rt(RtStmt::SetEffect(Effect::Ghost), vec![Expr::Num(25.0)]));
        assert!(matches!(&body[4], Stmt::Rt(RtStmt::Say, args) if matches!(&args[0], Expr::Rt(RtExpr::PropertyOf, _))));
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
    fn parameters_read_as_costumes_keep_their_text() {
        // `define pick (c)` switches to costume `c`, and the flag calls it with the text "2".
        let p = project(
            r#"{"d":{"opcode":"procedures_definition","next":"s","parent":null,"inputs":{"custom_block":[1,"pr"]},"fields":{},"topLevel":true},
            "pr":{"opcode":"procedures_prototype","next":null,"parent":"d","inputs":{"a1":[1,"ar"]},"fields":{},"shadow":true,"topLevel":false,
              "mutation":{"tagName":"mutation","children":[],"proccode":"pick %s","argumentids":"[\"a1\"]","argumentnames":"[\"c\"]","argumentdefaults":"[\"\"]","warp":"false"}},
            "ar":{"opcode":"argument_reporter_string_number","next":null,"parent":"pr","inputs":{},"fields":{"VALUE":["c",null]},"shadow":true,"topLevel":false},
            "s":{"opcode":"looks_switchcostumeto","next":null,"parent":"d","inputs":{"COSTUME":[3,"r",[10,""]]},"fields":{},"topLevel":false},
            "r":{"opcode":"argument_reporter_string_number","next":null,"parent":"s","inputs":{},"fields":{"VALUE":["c",null]},"topLevel":false},
            "h":{"opcode":"event_whenflagclicked","next":"c","parent":null,"inputs":{},"fields":{},"topLevel":true},
            "c":{"opcode":"procedures_call","next":null,"parent":"h","inputs":{"a1":[1,[10,"2"]]},"fields":{},"topLevel":false,
              "mutation":{"tagName":"mutation","children":[],"proccode":"pick %s","argumentids":"[\"a1\"]","warp":"false"}}}"#,
            "{}",
        );
        let lowered = lower(&p).unwrap();
        let pick = lowered.program.functions.iter().find(|f| f.name.ends_with("pick %s")).unwrap();
        assert_eq!(pick.params[0].ty, Type::Any, "{}", thatch_ir::dump(&lowered.program));
        let call = lowered.program.functions.iter().flat_map(|f| &f.body).find_map(|s| match s {
            Stmt::Call { args, .. } => Some(args[0].clone()),
            _ => None,
        });
        assert!(
            matches!(&call, Some(Expr::Cast(Type::Any, inner)) if matches!(**inner, Expr::Str(_))),
            "the text \"2\" is passed as text: {call:?}"
        );
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
