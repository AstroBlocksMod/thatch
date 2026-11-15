//! Lowers the Thatch IR to machine code with Cranelift.
//!
//! The same lowering serves two outputs. [`compile_jit`] compiles into memory and hands back the
//! program's init function, which `thatch run` passes to the runtime in the same process.
//! [`compile_object`] writes an object file with the same functions plus a C `main` that calls
//! `thatch_main`; linked against `libthatch_runtime.a`, that is a standalone executable.
//!
//! Generated code holds a number as an `f64`, a boolean as an `i8`, a string as a handle (a
//! pointer) owned by the thread's temporaries, and a dynamically typed value as a pointer to a
//! 16-byte `Value` either in variable storage or in a slot on the stack. Everything it cannot do with
//! an instruction is a call into the runtime, by the names in `thatch_abi::RT_FNS`.

use std::collections::HashMap;

use cranelift_codegen::ir::{
    AbiParam, BlockArg, FuncRef, InstBuilder, MemFlagsData, Signature, StackSlotData, StackSlotKind, Value as CValue,
    condcodes::FloatCC, types,
};
use cranelift_codegen::isa::OwnedTargetIsa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, DataId, FuncId as CFuncId, Linkage, Module, default_libcall_names};
use cranelift_object::{ObjectBuilder, ObjectModule};
use thatch_abi::{AbiType, HAT_BROADCAST, HAT_FLAG, TAG_BOOL, TAG_NUM, TAG_STR, ThreadCtx};
use thatch_ir::{BinOp, Const, Expr, FuncId, Hat, Program, RtExpr, RtStmt, Stmt, Type, UnOp, VarRef};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("this machine is not supported by Cranelift: {0}")]
    Host(String),
    #[error("Cranelift settings: {0}")]
    Settings(#[from] settings::SetError),
    #[error("Cranelift: {0}")]
    Module(Box<cranelift_module::ModuleError>),
    #[error("Cranelift could not compile {function}: {message}")]
    Codegen { function: String, message: String },
    #[error("object file: {0}")]
    Object(String),
}

impl From<cranelift_module::ModuleError> for Error {
    fn from(e: cranelift_module::ModuleError) -> Self {
        Error::Module(Box::new(e))
    }
}

/// A program compiled into memory.
pub struct JitProgram {
    module: Option<JITModule>,
    init: *const u8,
    abi_version: *const u8,
}

impl JitProgram {
    /// The address of the program's `thatch_program_init`.
    pub fn init_address(&self) -> *const u8 {
        self.init
    }

    /// The ABI version the program reports through its `thatch_program_abi_version`, which a host
    /// checks before calling the init function.
    pub fn abi_version(&self) -> u32 {
        // SAFETY: the address is the exported function the compiler just produced, with this
        // signature.
        let f = unsafe { std::mem::transmute::<*const u8, unsafe extern "C" fn() -> i32>(self.abi_version) };
        // SAFETY: as above; the function only returns a constant.
        unsafe { f() as u32 }
    }
}

impl Drop for JitProgram {
    fn drop(&mut self) {
        if let Some(module) = self.module.take() {
            // SAFETY: the runtime run that used the code has finished; nothing points into it.
            unsafe { module.free_memory() };
        }
    }
}

fn host_isa(pic: bool) -> Result<OwnedTargetIsa, Error> {
    let mut flags = settings::builder();
    flags.set("opt_level", "speed")?;
    flags.set("is_pic", if pic { "true" } else { "false" })?;
    if !pic {
        flags.set("use_colocated_libcalls", "false")?;
    }
    let builder = cranelift_native::builder().map_err(|e| Error::Host(e.to_owned()))?;
    builder.finish(settings::Flags::new(flags)).map_err(|e| Error::Host(e.to_string()))
}

/// Compiles the program into memory. `symbols` are the runtime functions by name, as
/// `thatch_runtime::symbols()` lists them.
pub fn compile_jit(program: &Program, symbols: &[(&str, *const u8)]) -> Result<JitProgram, Error> {
    let isa = host_isa(false)?;
    let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
    builder.symbols(symbols.iter().map(|(name, ptr)| (*name, *ptr)));
    let mut module = JITModule::new(builder);
    let (init, version) = Compiler::new(&mut module, program).compile(false)?;
    module.finalize_definitions()?;
    let init = module.get_finalized_function(init);
    let abi_version = module.get_finalized_function(version);
    Ok(JitProgram { module: Some(module), init, abi_version })
}

/// Compiles the program into an object file, ready to link against the runtime. With `with_main`
/// it includes a C `main` that runs the program on the headless host; without, a host provides
/// `main` and drives the program through `thatch_program_init` and `thatch_program_abi_version`.
pub fn compile_object(program: &Program, with_main: bool) -> Result<Vec<u8>, Error> {
    let isa = host_isa(true)?;
    let builder = ObjectBuilder::new(isa, "thatch_program", default_libcall_names())?;
    let mut module = ObjectModule::new(builder);
    Compiler::new(&mut module, program).compile(with_main)?;
    module.finish().emit().map_err(|e| Error::Object(e.to_string()))
}

fn cl_type(t: AbiType) -> types::Type {
    match t {
        AbiType::F64 => types::F64,
        AbiType::I64 => types::I64,
        AbiType::I8 => types::I8,
        AbiType::I32 => types::I32,
    }
}

fn param_type(t: Type) -> types::Type {
    match t {
        Type::Num => types::F64,
        Type::Bool => types::I8,
        Type::Str | Type::Any => types::I64,
    }
}

/// Compiles one program into a module.
struct Compiler<'a, M: Module> {
    module: &'a mut M,
    program: &'a Program,
    rt: HashMap<&'static str, CFuncId>,
    funcs: Vec<CFuncId>,
    data: HashMap<String, DataId>,
}

impl<'a, M: Module> Compiler<'a, M> {
    fn new(module: &'a mut M, program: &'a Program) -> Self {
        Self { module, program, rt: HashMap::new(), funcs: Vec::new(), data: HashMap::new() }
    }

    /// Compiles everything and returns the ids of `thatch_program_init` and
    /// `thatch_program_abi_version`.
    fn compile(&mut self, with_main: bool) -> Result<(CFuncId, CFuncId), Error> {
        for f in thatch_abi::RT_FNS {
            let mut sig = self.module.make_signature();
            for p in f.params {
                sig.params.push(AbiParam::new(cl_type(*p)));
            }
            if let Some(r) = f.ret {
                sig.returns.push(AbiParam::new(cl_type(r)));
            }
            let id = self.module.declare_function(f.name, Linkage::Import, &sig)?;
            self.rt.insert(f.name, id);
        }
        for (i, f) in self.program.functions.iter().enumerate() {
            let sig = self.function_signature(f);
            let id = self.module.declare_function(&format!("thatch_fn_{i}"), Linkage::Local, &sig)?;
            self.funcs.push(id);
        }
        for (i, f) in self.program.functions.iter().enumerate() {
            self.compile_function(i, f)?;
        }
        let init = self.compile_init()?;
        let version = self.compile_abi_version()?;
        if with_main {
            self.compile_main(init)?;
        }
        Ok((init, version))
    }

    fn function_signature(&self, f: &thatch_ir::Function) -> Signature {
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        for p in &f.params {
            sig.params.push(AbiParam::new(param_type(p.ty)));
        }
        sig
    }

    /// A read-only data object holding the bytes of `s`, shared between equal strings.
    fn data(&mut self, s: &str) -> Result<DataId, Error> {
        if let Some(id) = self.data.get(s) {
            return Ok(*id);
        }
        let name = format!("thatch_str_{}", self.data.len());
        let id = self.module.declare_data(&name, Linkage::Local, false, false)?;
        let mut desc = DataDescription::new();
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0);
        desc.define(bytes.into_boxed_slice());
        self.module.define_data(id, &desc)?;
        self.data.insert(s.to_owned(), id);
        Ok(id)
    }

    fn define(&mut self, id: CFuncId, name: &str, ctx: &mut cranelift_codegen::Context) -> Result<(), Error> {
        if let Err(e) = self.module.define_function(id, ctx) {
            // The module error only says "verifier errors"; the verifier itself says where.
            let message = match cranelift_codegen::verifier::verify_function(&ctx.func, self.module.isa()) {
                Err(errors) => cranelift_codegen::print_errors::pretty_verifier_error(&ctx.func, None, errors),
                Ok(()) => e.to_string(),
            };
            return Err(Error::Codegen { function: name.to_owned(), message });
        }
        self.module.clear_context(ctx);
        Ok(())
    }

    fn compile_function(&mut self, index: usize, f: &thatch_ir::Function) -> Result<(), Error> {
        let mut ctx = self.module.make_context();
        ctx.func.signature = self.function_signature(f);
        let mut fb_ctx = FunctionBuilderContext::new();
        {
            let builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
            let mut g = FnGen::new(self, builder, f);
            g.body(&f.body);
            g.finish();
        }
        self.define(self.funcs[index], &f.name, &mut ctx)
    }

    /// `thatch_program_init(rt)`: registers constants, targets, variables and scripts.
    fn compile_init(&mut self) -> Result<CFuncId, Error> {
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        let id = self.module.declare_function(thatch_abi::PROGRAM_INIT, Linkage::Export, &sig)?;
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut fb_ctx = FunctionBuilderContext::new();

        // Collect what the body needs before borrowing the function builder.
        let mut strings = Vec::new();
        for s in &self.program.strings {
            strings.push((self.data(s)?, s.len()));
        }
        struct TargetData {
            name: (DataId, usize),
            is_stage: bool,
            nums: Vec<((DataId, usize), f64)>,
            anys: Vec<((DataId, usize), u64, u64)>,
            scripts: Vec<(i64, (DataId, usize), CFuncId)>,
        }
        let mut targets = Vec::new();
        for t in &self.program.targets {
            let mut nums = Vec::new();
            for v in &t.num_vars {
                nums.push(((self.data(&v.name)?, v.name.len()), v.initial));
            }
            let mut anys = Vec::new();
            for v in &t.any_vars {
                let (tag, bits) = match &v.initial {
                    Const::Num(n) => (TAG_NUM, n.to_bits()),
                    Const::Str(s) => (TAG_STR, u64::from(s.0)),
                    Const::Bool(b) => (TAG_BOOL, u64::from(*b)),
                };
                anys.push(((self.data(&v.name)?, v.name.len()), tag, bits));
            }
            let mut scripts = Vec::new();
            for s in &t.scripts {
                let (kind, arg) = match &s.hat {
                    Hat::FlagClicked => (HAT_FLAG, String::new()),
                    Hat::BroadcastReceived(name) => (HAT_BROADCAST, name.clone()),
                };
                scripts.push((kind, (self.data(&arg)?, arg.len()), self.funcs[s.func.0 as usize]));
            }
            targets.push(TargetData {
                name: (self.data(&t.name)?, t.name.len()),
                is_stage: t.is_stage,
                nums,
                anys,
                scripts,
            });
        }

        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let rt = b.block_params(entry)[0];
            let mut refs: HashMap<&str, FuncRef> = HashMap::new();
            let mut rt_ref = |b: &mut FunctionBuilder, module: &mut M, name: &'static str| -> FuncRef {
                *refs.entry(name).or_insert_with(|| module.declare_func_in_func(self.rt[name], b.func))
            };
            let addr = |b: &mut FunctionBuilder, module: &mut M, d: (DataId, usize)| -> (CValue, CValue) {
                let gv = module.declare_data_in_func(d.0, b.func);
                let p = b.ins().symbol_value(types::I64, gv);
                let len = b.ins().iconst(types::I64, d.1 as i64);
                (p, len)
            };
            for (d, len) in strings {
                let f = rt_ref(&mut b, self.module, thatch_abi::ADD_CONST);
                let (p, l) = addr(&mut b, self.module, (d, len));
                b.ins().call(f, &[rt, p, l]);
            }
            for t in targets {
                let f = rt_ref(&mut b, self.module, thatch_abi::ADD_TARGET);
                let (p, l) = addr(&mut b, self.module, t.name);
                let is_stage = b.ins().iconst(types::I8, i64::from(t.is_stage));
                let call = b.ins().call(f, &[rt, p, l, is_stage]);
                let target = b.inst_results(call)[0];
                for (name, initial) in t.nums {
                    let f = rt_ref(&mut b, self.module, thatch_abi::ADD_NUM_VAR);
                    let (p, l) = addr(&mut b, self.module, name);
                    let v = b.ins().f64const(initial);
                    b.ins().call(f, &[rt, target, p, l, v]);
                }
                for (name, tag, bits) in t.anys {
                    let f = rt_ref(&mut b, self.module, thatch_abi::ADD_ANY_VAR);
                    let (p, l) = addr(&mut b, self.module, name);
                    let tag = b.ins().iconst(types::I64, tag as i64);
                    let bits = b.ins().iconst(types::I64, bits as i64);
                    b.ins().call(f, &[rt, target, p, l, tag, bits]);
                }
                for (kind, arg, func) in t.scripts {
                    let f = rt_ref(&mut b, self.module, thatch_abi::ADD_SCRIPT);
                    let (p, l) = addr(&mut b, self.module, arg);
                    let kind = b.ins().iconst(types::I64, kind);
                    let fref = self.module.declare_func_in_func(func, b.func);
                    let fp = b.ins().func_addr(types::I64, fref);
                    b.ins().call(f, &[rt, target, kind, p, l, fp]);
                }
            }
            b.ins().return_(&[]);
            b.seal_all_blocks();
            b.finalize(self.module.isa().frontend_config());
        }
        self.define(id, thatch_abi::PROGRAM_INIT, &mut ctx)?;
        Ok(id)
    }

    /// `thatch_program_abi_version()`: the ABI version the program was compiled against.
    fn compile_abi_version(&mut self) -> Result<CFuncId, Error> {
        let mut sig = self.module.make_signature();
        sig.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function(thatch_abi::PROGRAM_ABI_VERSION, Linkage::Export, &sig)?;
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut fb_ctx = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
            let entry = b.create_block();
            b.switch_to_block(entry);
            let version = b.ins().iconst(types::I32, i64::from(thatch_abi::ABI_VERSION));
            b.ins().return_(&[version]);
            b.seal_all_blocks();
            b.finalize(self.module.isa().frontend_config());
        }
        self.define(id, thatch_abi::PROGRAM_ABI_VERSION, &mut ctx)?;
        Ok(id)
    }

    /// `main(argc, argv)`: hands the program to the headless host.
    fn compile_main(&mut self, init: CFuncId) -> Result<(), Error> {
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I32));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I32));
        let id = self.module.declare_function("main", Linkage::Export, &sig)?;
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;
        let mut fb_ctx = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let argc = b.block_params(entry)[0];
            let argv = b.block_params(entry)[1];
            let init_ref = self.module.declare_func_in_func(init, b.func);
            let init_addr = b.ins().func_addr(types::I64, init_ref);
            let version = b.ins().iconst(types::I32, i64::from(thatch_abi::ABI_VERSION));
            let main = self.module.declare_func_in_func(self.rt[thatch_abi::MAIN], b.func);
            let call = b.ins().call(main, &[argc, argv, init_addr, version]);
            let code = b.inst_results(call)[0];
            b.ins().return_(&[code]);
            b.seal_all_blocks();
            b.finalize(self.module.isa().frontend_config());
        }
        self.define(id, "main", &mut ctx)
    }
}

const TRUSTED: MemFlagsData = MemFlagsData::trusted();

fn offset(field: usize) -> i32 {
    field as i32
}

/// Generates the body of one IR function.
struct FnGen<'a, 'b, M: Module> {
    c: &'a mut Compiler<'b, M>,
    b: FunctionBuilder<'a>,
    ctx: CValue,
    params: Vec<CValue>,
    param_types: Vec<Type>,
    rt: HashMap<&'static str, FuncRef>,
    callees: HashMap<u32, FuncRef>,
    /// Exit blocks of the enclosing loops.
    loops: Vec<cranelift_codegen::ir::Block>,
    /// Whether the current block still needs a terminator.
    open: bool,
    /// Whether a runtime call in the current statement made a string.
    temps: bool,
}

impl<'a, 'b, M: Module> FnGen<'a, 'b, M> {
    fn new(c: &'a mut Compiler<'b, M>, mut b: FunctionBuilder<'a>, f: &thatch_ir::Function) -> Self {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let values = b.block_params(entry).to_vec();
        Self {
            c,
            b,
            ctx: values[0],
            params: values[1..].to_vec(),
            param_types: f.params.iter().map(|p| p.ty).collect(),
            rt: HashMap::new(),
            callees: HashMap::new(),
            loops: Vec::new(),
            open: true,
            temps: false,
        }
    }

    fn finish(mut self) {
        if self.open {
            self.b.ins().return_(&[]);
        }
        self.b.seal_all_blocks();
        self.b.finalize(self.c.module.isa().frontend_config());
    }

    fn rt_ref(&mut self, name: &'static str) -> FuncRef {
        if let Some(r) = self.rt.get(name) {
            return *r;
        }
        let r = self.c.module.declare_func_in_func(self.c.rt[name], self.b.func);
        self.rt.insert(name, r);
        r
    }

    fn call_rt(&mut self, name: &'static str, args: &[CValue]) -> Option<CValue> {
        let f = self.rt_ref(name);
        let inst = self.b.ins().call(f, args);
        self.b.inst_results(inst).first().copied()
    }

    fn rt_value(&mut self, name: &'static str, args: &[CValue]) -> CValue {
        self.call_rt(name, args).expect("runtime function returns a value")
    }

    fn ty(&self, e: &Expr) -> Type {
        e.ty(&|v| match v {
            VarRef::TargetNum(_) | VarRef::StageNum(_) => Type::Num,
            VarRef::TargetAny(_) | VarRef::StageAny(_) => Type::Any,
            VarRef::Param(i) => self.param_types.get(i as usize).copied().unwrap_or(Type::Any),
        })
    }

    fn new_block(&mut self) -> cranelift_codegen::ir::Block {
        self.b.create_block()
    }

    fn switch(&mut self, block: cranelift_codegen::ir::Block) {
        self.b.switch_to_block(block);
        self.open = true;
    }

    fn jump(&mut self, block: cranelift_codegen::ir::Block) {
        if self.open {
            self.b.ins().jump(block, &[]);
            self.open = false;
        }
    }

    /// Ends the current block with a return and continues in a fresh, unreachable block.
    fn return_now(&mut self) {
        if self.open {
            self.b.ins().return_(&[]);
            self.open = false;
        }
        let dead = self.new_block();
        self.switch(dead);
    }

    /// Returns from the function if the thread was killed while the last call yielded.
    fn check_killed(&mut self) {
        let killed = self.b.ins().load(types::I64, TRUSTED, self.ctx, offset(std::mem::offset_of!(ThreadCtx, killed)));
        let ret = self.new_block();
        let go_on = self.new_block();
        self.b.ins().brif(killed, ret, &[], go_on, &[]);
        self.open = false;
        self.switch(ret);
        self.b.ins().return_(&[]);
        self.open = false;
        self.switch(go_on);
    }

    fn clear_temps(&mut self) {
        if self.temps {
            self.call_rt(thatch_abi::TEMPS_CLEAR, &[self.ctx]);
            self.temps = false;
        }
    }

    // ---- variables ------------------------------------------------------------------------------

    fn ctx_field(&mut self, field: usize) -> CValue {
        self.b.ins().load(types::I64, TRUSTED, self.ctx, offset(field))
    }

    fn num_var_addr(&mut self, v: VarRef) -> (CValue, i32) {
        match v {
            VarRef::TargetNum(i) => (self.ctx_field(std::mem::offset_of!(ThreadCtx, nums)), (i * 8) as i32),
            VarRef::StageNum(i) => (self.ctx_field(std::mem::offset_of!(ThreadCtx, stage_nums)), (i * 8) as i32),
            _ => unreachable!("not a numeric variable"),
        }
    }

    fn any_var_addr(&mut self, v: VarRef) -> CValue {
        let (base, i) = match v {
            VarRef::TargetAny(i) => (self.ctx_field(std::mem::offset_of!(ThreadCtx, anys)), i),
            VarRef::StageAny(i) => (self.ctx_field(std::mem::offset_of!(ThreadCtx, stage_anys)), i),
            _ => unreachable!("not a dynamically typed variable"),
        };
        self.b.ins().iadd_imm_s(base, i64::from(i) * 16)
    }

    /// A 16-byte stack slot holding a `Value`.
    fn value_slot(&mut self) -> CValue {
        let slot = self.b.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 16, 3));
        self.b.ins().stack_addr(types::I64, slot, 0)
    }

    /// Boxes a value into a stack `Value` and returns its address.
    fn box_any(&mut self, v: CValue, ty: Type) -> CValue {
        if ty == Type::Any {
            return v;
        }
        let p = self.value_slot();
        let (tag, bits) = match ty {
            Type::Num => (TAG_NUM, self.b.ins().bitcast(types::I64, MemFlagsData::new(), v)),
            Type::Str => (TAG_STR, v),
            Type::Bool => (TAG_BOOL, self.b.ins().uextend(types::I64, v)),
            Type::Any => unreachable!(),
        };
        let tag = self.b.ins().iconst(types::I64, tag as i64);
        self.b.ins().store(TRUSTED, tag, p, 0);
        self.b.ins().store(TRUSTED, bits, p, 8);
        p
    }

    // ---- expressions ----------------------------------------------------------------------------

    fn expr(&mut self, e: &Expr) -> CValue {
        match e {
            Expr::Num(n) => self.b.ins().f64const(*n),
            Expr::Bool(b) => self.b.ins().iconst(types::I8, i64::from(*b)),
            Expr::Str(s) => {
                let consts = self.ctx_field(std::mem::offset_of!(ThreadCtx, consts));
                self.b.ins().load(types::I64, TRUSTED, consts, (s.0 * 8) as i32)
            }
            Expr::Var(v) => match v {
                VarRef::TargetNum(_) | VarRef::StageNum(_) => {
                    let (base, off) = self.num_var_addr(*v);
                    self.b.ins().load(types::F64, TRUSTED, base, off)
                }
                VarRef::TargetAny(_) | VarRef::StageAny(_) => self.any_var_addr(*v),
                VarRef::Param(i) => self.params[*i as usize],
            },
            Expr::Cast(to, inner) => {
                let from = self.ty(inner);
                let v = self.expr(inner);
                self.cast(v, from, *to)
            }
            Expr::Unary(op, inner) => {
                let v = self.expr(inner);
                match op {
                    UnOp::Not => self.b.ins().bxor_imm_s(v, 1),
                    UnOp::ToNumber => {
                        // `Cast.toNumber`: NaN becomes 0.
                        let nan = self.b.ins().fcmp(FloatCC::Unordered, v, v);
                        let zero = self.b.ins().f64const(0.0);
                        self.b.ins().select(nan, zero, v)
                    }
                    UnOp::Round => self.rt_value(thatch_abi::ROUND, &[v]),
                    UnOp::Abs => self.b.ins().fabs(v),
                    UnOp::Floor => self.b.ins().floor(v),
                    UnOp::Ceil => self.b.ins().ceil(v),
                    UnOp::Sqrt => self.b.ins().sqrt(v),
                    UnOp::MathOp(i) => {
                        let op = self.b.ins().iconst(types::I64, i64::from(*i));
                        self.rt_value(thatch_abi::MATHOP, &[op, v])
                    }
                }
            }
            Expr::Binary(op, l, r) => self.binary(*op, l, r),
            Expr::Rt(f, args) => self.rt_expr(*f, args),
        }
    }

    fn cast(&mut self, v: CValue, from: Type, to: Type) -> CValue {
        if from == to {
            return v;
        }
        match (from, to) {
            (Type::Num, Type::Str) => {
                self.temps = true;
                self.rt_value(thatch_abi::NUM_TO_STR, &[self.ctx, v])
            }
            (Type::Num, Type::Bool) => {
                let zero = self.b.ins().f64const(0.0);
                self.b.ins().fcmp(FloatCC::OrderedNotEqual, v, zero)
            }
            (Type::Str, Type::Num) => self.rt_value(thatch_abi::STR_TO_NUM, &[v]),
            (Type::Str, Type::Bool) => self.rt_value(thatch_abi::STR_TO_BOOL, &[v]),
            (Type::Bool, Type::Num) => {
                let one = self.b.ins().f64const(1.0);
                let zero = self.b.ins().f64const(0.0);
                self.b.ins().select(v, one, zero)
            }
            (Type::Bool, Type::Str) => {
                self.temps = true;
                self.rt_value(thatch_abi::BOOL_TO_STR, &[self.ctx, v])
            }
            (Type::Any, Type::Num) => self.rt_value(thatch_abi::ANY_TO_NUM, &[v]),
            (Type::Any, Type::Str) => {
                self.temps = true;
                self.rt_value(thatch_abi::ANY_TO_STR, &[self.ctx, v])
            }
            (Type::Any, Type::Bool) => self.rt_value(thatch_abi::ANY_TO_BOOL, &[v]),
            (_, Type::Any) => self.box_any(v, from),
            _ => unreachable!("cast {from} to {to}"),
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr) -> CValue {
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                let a = self.num(l);
                let b = self.num(r);
                match op {
                    BinOp::Add => self.b.ins().fadd(a, b),
                    BinOp::Sub => self.b.ins().fsub(a, b),
                    BinOp::Mul => self.b.ins().fmul(a, b),
                    BinOp::Div => self.b.ins().fdiv(a, b),
                    _ => self.rt_value(thatch_abi::MOD, &[a, b]),
                }
            }
            BinOp::And | BinOp::Or => {
                let a = self.boolean(l);
                let b = self.boolean(r);
                if op == BinOp::And { self.b.ins().band(a, b) } else { self.b.ins().bor(a, b) }
            }
            BinOp::Lt | BinOp::Gt | BinOp::Eq => {
                let cc = match op {
                    BinOp::Lt => FloatCC::LessThan,
                    BinOp::Gt => FloatCC::GreaterThan,
                    _ => FloatCC::Equal,
                };
                let (lt, rt) = (self.ty(l), self.ty(r));
                let a = self.expr(l);
                let b = self.expr(r);
                if lt == Type::Num && rt == Type::Num {
                    return self.compare_nums(cc, a, b);
                }
                let pa = self.box_any(a, lt);
                let pb = self.box_any(b, rt);
                let c = self.rt_value(thatch_abi::COMPARE, &[pa, pb]);
                let zero = self.b.ins().f64const(0.0);
                self.b.ins().fcmp(cc, c, zero)
            }
        }
    }

    /// Compares two numbers. When neither is NaN an ordered comparison matches `Cast.compare`;
    /// otherwise the runtime compares them as strings.
    fn compare_nums(&mut self, cc: FloatCC, a: CValue, b: CValue) -> CValue {
        let ordered = self.b.ins().fcmp(FloatCC::Ordered, a, b);
        let fast = self.new_block();
        let slow = self.new_block();
        let merge = self.new_block();
        let result = self.b.append_block_param(merge, types::I8);
        self.b.ins().brif(ordered, fast, &[], slow, &[]);
        self.switch(fast);
        let r = self.b.ins().fcmp(cc, a, b);
        self.b.ins().jump(merge, &[BlockArg::Value(r)]);
        self.switch(slow);
        let c = self.rt_value(thatch_abi::COMPARE_NUMS, &[a, b]);
        let zero = self.b.ins().f64const(0.0);
        let r = self.b.ins().fcmp(cc, c, zero);
        self.b.ins().jump(merge, &[BlockArg::Value(r)]);
        self.switch(merge);
        result
    }

    fn typed(&mut self, e: &Expr, want: Type) -> CValue {
        let from = self.ty(e);
        let v = self.expr(e);
        self.cast(v, from, want)
    }

    fn num(&mut self, e: &Expr) -> CValue {
        self.typed(e, Type::Num)
    }

    fn boolean(&mut self, e: &Expr) -> CValue {
        self.typed(e, Type::Bool)
    }

    fn string(&mut self, e: &Expr) -> CValue {
        self.typed(e, Type::Str)
    }

    fn any(&mut self, e: &Expr) -> CValue {
        self.typed(e, Type::Any)
    }

    fn rt_expr(&mut self, f: RtExpr, args: &[Expr]) -> CValue {
        match f {
            RtExpr::Join => {
                let a = self.string(&args[0]);
                let b = self.string(&args[1]);
                self.temps = true;
                self.rt_value(thatch_abi::JOIN, &[self.ctx, a, b])
            }
            RtExpr::LetterOf => {
                let i = self.num(&args[0]);
                let s = self.string(&args[1]);
                self.temps = true;
                self.rt_value(thatch_abi::LETTER_OF, &[self.ctx, i, s])
            }
            RtExpr::Length => {
                let s = self.string(&args[0]);
                self.rt_value(thatch_abi::LENGTH, &[s])
            }
            RtExpr::Contains => {
                let a = self.string(&args[0]);
                let b = self.string(&args[1]);
                self.rt_value(thatch_abi::CONTAINS, &[a, b])
            }
            RtExpr::Random => {
                let a = self.any(&args[0]);
                let b = self.any(&args[1]);
                self.rt_value(thatch_abi::RANDOM, &[self.ctx, a, b])
            }
            RtExpr::Timer => self.rt_value(thatch_abi::TIMER, &[self.ctx]),
        }
    }

    // ---- statements -----------------------------------------------------------------------------

    fn body(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Set(var, e) => {
                match var {
                    VarRef::TargetNum(_) | VarRef::StageNum(_) => {
                        let v = self.num(e);
                        let (base, off) = self.num_var_addr(*var);
                        self.b.ins().store(TRUSTED, v, base, off);
                    }
                    VarRef::TargetAny(_) | VarRef::StageAny(_) => {
                        let ty = self.ty(e);
                        let v = self.expr(e);
                        let slot = self.any_var_addr(*var);
                        match ty {
                            Type::Num => self.call_rt(thatch_abi::VAR_SET_NUM, &[slot, v]),
                            Type::Str => self.call_rt(thatch_abi::VAR_SET_STR, &[slot, v]),
                            Type::Bool => self.call_rt(thatch_abi::VAR_SET_BOOL, &[slot, v]),
                            Type::Any => self.call_rt(thatch_abi::VAR_SET, &[slot, v]),
                        };
                    }
                    VarRef::Param(_) => {}
                }
                self.clear_temps();
            }
            Stmt::If { cond, then, els } => {
                let c = self.boolean(cond);
                self.clear_temps();
                let then_b = self.new_block();
                let else_b = self.new_block();
                let merge = self.new_block();
                self.b.ins().brif(c, then_b, &[], else_b, &[]);
                self.open = false;
                self.switch(then_b);
                self.body(then);
                self.jump(merge);
                self.switch(else_b);
                self.body(els);
                self.jump(merge);
                self.switch(merge);
            }
            Stmt::Repeat { times, body } => {
                let t = self.num(times);
                self.clear_temps();
                let count = self.rt_value(thatch_abi::ROUND, &[t]);
                let counter = self.b.declare_var(types::F64);
                self.b.def_var(counter, count);
                let header = self.new_block();
                let body_b = self.new_block();
                let exit = self.new_block();
                self.jump(header);
                self.switch(header);
                let c = self.b.use_var(counter);
                let one = self.b.ins().f64const(1.0);
                let c = self.b.ins().fsub(c, one);
                self.b.def_var(counter, c);
                let zero = self.b.ins().f64const(0.0);
                let go = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, c, zero);
                self.b.ins().brif(go, body_b, &[], exit, &[]);
                self.open = false;
                self.switch(body_b);
                self.loops.push(exit);
                self.body(body);
                self.loops.pop();
                self.jump(header);
                self.switch(exit);
            }
            Stmt::Loop(body) => {
                let header = self.new_block();
                let exit = self.new_block();
                self.jump(header);
                self.switch(header);
                self.loops.push(exit);
                self.body(body);
                self.loops.pop();
                self.jump(header);
                self.switch(exit);
            }
            Stmt::Break => {
                let exit = *self.loops.last().expect("break outside a loop");
                self.jump(exit);
                let dead = self.new_block();
                self.switch(dead);
            }
            Stmt::Yield => {
                // Always a call, even in a warp function: the runtime returns at once in warp
                // mode until the 500 ms limit, and only it knows whether the thread is in warp
                // mode, since a block called from a warp block inherits it.
                self.call_rt(thatch_abi::YIELD, &[self.ctx]);
                self.check_killed();
            }
            Stmt::Call { func, args, nesting } => self.call(*func, args, *nesting),
            Stmt::Rt(f, args) => self.rt_stmt(*f, args),
            Stmt::Return => self.return_now(),
        }
    }

    fn call(&mut self, func: FuncId, args: &[Expr], nesting: u32) {
        let callee = &self.c.program.functions[func.0 as usize];
        let warp = callee.warp;
        let param_types: Vec<Type> = callee.params.iter().map(|p| p.ty).collect();
        let mut values = vec![self.ctx];
        for (arg, ty) in args.iter().zip(param_types) {
            let mut v = self.typed(arg, ty);
            if ty == Type::Any && matches!(arg, Expr::Var(VarRef::TargetAny(_) | VarRef::StageAny(_))) {
                // The callee gets the value as it is now, not the variable, which the callee or
                // another thread may change before the parameter is read.
                let slot = self.value_slot();
                self.call_rt(thatch_abi::ANY_SNAPSHOT, &[self.ctx, slot, v]);
                self.temps = true;
                v = slot;
            }
            values.push(v);
        }
        let id = self.b.ins().iconst(types::I64, i64::from(func.0));
        let nesting = self.b.ins().iconst(types::I64, i64::from(nesting));
        let warp_flag = self.b.ins().iconst(types::I8, i64::from(warp));
        self.call_rt(thatch_abi::PROC_ENTER, &[self.ctx, id, nesting, warp_flag]);
        self.check_killed();
        let fref = match self.callees.get(&func.0) {
            Some(r) => *r,
            None => {
                let r = self.c.module.declare_func_in_func(self.c.funcs[func.0 as usize], self.b.func);
                self.callees.insert(func.0, r);
                r
            }
        };
        self.b.ins().call(fref, &values);
        self.call_rt(thatch_abi::PROC_EXIT, &[self.ctx, warp_flag]);
        self.check_killed();
        self.clear_temps();
    }

    fn rt_stmt(&mut self, f: RtStmt, args: &[Expr]) {
        match f {
            RtStmt::Say | RtStmt::Think => {
                let m = self.any(&args[0]);
                let think = self.b.ins().iconst(types::I8, i64::from(f == RtStmt::Think));
                self.call_rt(thatch_abi::SAY, &[self.ctx, m, think]);
                self.clear_temps();
            }
            RtStmt::SayFor | RtStmt::ThinkFor => {
                let m = self.any(&args[0]);
                let secs = self.num(&args[1]);
                let think = self.b.ins().iconst(types::I8, i64::from(f == RtStmt::ThinkFor));
                self.call_rt(thatch_abi::SAY_FOR, &[self.ctx, m, secs, think]);
                self.clear_temps();
                self.check_killed();
            }
            RtStmt::Wait => {
                let secs = self.num(&args[0]);
                self.clear_temps();
                self.call_rt(thatch_abi::WAIT, &[self.ctx, secs]);
                self.check_killed();
            }
            RtStmt::Broadcast => {
                let name = self.string(&args[0]);
                self.call_rt(thatch_abi::BROADCAST, &[self.ctx, name]);
                self.clear_temps();
            }
            RtStmt::BroadcastAndWait => {
                let name = self.string(&args[0]);
                self.call_rt(thatch_abi::BROADCAST_WAIT, &[self.ctx, name]);
                self.clear_temps();
                self.check_killed();
            }
            RtStmt::StopAll => {
                self.call_rt(thatch_abi::STOP_ALL, &[self.ctx]);
                self.return_now();
            }
            RtStmt::StopOthers => {
                self.call_rt(thatch_abi::STOP_OTHERS, &[self.ctx]);
            }
            RtStmt::ResetTimer => {
                self.call_rt(thatch_abi::RESET_TIMER, &[self.ctx]);
            }
        }
    }
}

#[allow(dead_code)]
fn variable_unused(_: Variable) {}
