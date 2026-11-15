# The IR

The Thatch IR is what frontends produce and backends consume. It lives in the `thatch-ir` crate and `thatch dump` prints it.

It is a structured, typed representation rather than a flat SSA form: a function body is a tree of statements, loops are real loops, and conditionals are real conditionals. That keeps the frontends simple and leaves the SSA construction to Cranelift, which does it well. Should a pass ever need SSA, the IR can be converted; nothing in the design depends on it staying structured.

## Types

Every expression has a static type:

- `Num`: an IEEE double, the only number Scratch has.
- `Str`: a string.
- `Bool`: a boolean.
- `Any`: a dynamically typed Scratch value, one of the three above, decided at run time.

Conversions are explicit `Cast` expressions. The frontend inserts them wherever a block needs an input of some type, so a backend never has to guess, and a pass can see every conversion it might remove. The rules of each conversion are those of scratch-vm's `Cast`, in the `thatch-values` crate.

## Programs

A program has targets (the stage first, then the sprites back to front), a string constant pool, and functions. Each target has its numeric variables, its dynamically typed variables, its scripts, each of which is a hat and a function, its costume names, and the state it starts in (`SpriteState`: position, direction, size, costume, visibility, rotation style, draggability and volume). A function has typed parameters, a body, and a `warp` flag for custom blocks that run without screen refresh.

Variables are addressed as `TargetNum`, `TargetAny`, `StageNum` or `StageAny` with an index, or as `Param` for a function parameter. A sprite reaching for a global variable uses the stage forms.

## Statements

`Set`
: Stores an expression in a variable. A numeric variable takes a `Num`; a dynamically typed one takes anything.

`If`
: A conditional with a `Bool` condition and two bodies.

`Repeat`
: A counted loop. The count is rounded once; each iteration first decrements the counter and runs the body only while the counter is still at least 0, which is how scratch-vm's `repeat` treats NaN (never runs) and infinity (runs forever).

`Loop` and `Break`
: An endless loop that only `Break` leaves. `repeat until`, `while` and `forever` are built from these.

`Yield`
: The end of a loop iteration in cooperative code. The runtime decides whether anything happens: in warp mode it returns at once. Loops in a warp function carry no `Yield` at all.

`Call`
: Calls a function with arguments of the parameter types. It carries the nesting depth of the call site, which the runtime's recursion rule needs.

`Rt`
: A runtime command: `say`, `wait`, `broadcast`, `stop all` and the like, and the motion and looks commands that change a sprite's state, which lives in the runtime (`MoveSteps`, `GoToXY`, `Turn`, `Glide`, `SwitchCostume`, `SetEffect`, `GoToLayer` and so on). The IR says what is called and with which argument types; the ABI says how. Turning left and going backward a number of layers are the right and forward forms with the amount negated, which is exact in IEEE arithmetic.

`Return`
: Leaves the function. In a script that ends the thread; in a custom block it returns to the caller, which is what `stop this script` does inside a custom block.

## Expressions

Literals, variables, casts, unary and binary operators, and runtime reporters (`join`, `letter of`, `length of`, `contains`, `pick random`, `timer`, the sprite's position, direction and size, costume and backdrop numbers and names, and `PropertyOf`, the `of` block, which is the one reporter of type `Any`: it looks a property or variable up by name at run time).

The comparison operators take operands of any type and compare them as scratch-vm's `Cast.compare` does. A backend may compare two `Num` operands directly, as the Cranelift backend does, as long as NaN still compares as the string "nan".

## What a frontend promises

- Every `Cast` is explicit; the backend never converts on its own.
- A `Set` to a numeric variable stores a `Num`.
- `Break` appears only inside a loop, and `Yield` only where a thread may be switched.
- `Call` arguments match the callee's parameter types.

## What a backend promises

- Statements and expressions are evaluated in order, left to right. (The `and` and `or` operators may evaluate both sides.)
- After any call that may yield, the generated code checks whether its thread was stopped and returns if so.
- Strings produced during a statement live until the end of that statement.
