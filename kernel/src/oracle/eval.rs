//! The evaluator.
//!
//! Tail position in `if`, `begin`, `cond` and lambda bodies is handled by
//! looping rather than recursing, so tail-recursive ORACLE code runs in
//! constant stack. That matters here: kernel threads get 64 KiB of stack and
//! a runaway recursion would smash into whatever is below it.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cell::RefCell;

use super::value::{Env, EvalResult, Lambda, Value};

/// Guard against runaway non-tail recursion.
const MAX_DEPTH: u32 = 256;
/// Guard against `(while true ...)` locking up the desktop thread.
const MAX_ITERATIONS: u64 = 5_000_000;

pub struct Interpreter {
    pub global: Env,
    depth: u32,
    /// Everything the program printed during this evaluation.
    pub output: Vec<String>,
}

impl Interpreter {
    pub fn new() -> Self {
        let global = Env::root();
        let mut interpreter = Self {
            global,
            depth: 0,
            output: Vec::new(),
        };
        super::builtins::install(&mut interpreter);
        interpreter
    }

    pub fn print(&mut self, text: String) {
        self.output.push(text);
    }

    pub fn take_output(&mut self) -> Vec<String> {
        core::mem::take(&mut self.output)
    }

    /// Read and evaluate every form in `source`, returning the last value.
    pub fn run(&mut self, source: &str) -> EvalResult {
        let forms = super::reader::Reader::new(source).read_all()?;
        let mut last = Value::Nil;
        let global = self.global.clone();
        for form in forms {
            last = self.eval(form, &global)?;
        }
        Ok(last)
    }

    pub fn eval(&mut self, form: Value, env: &Env) -> EvalResult {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err("recursion too deep (ORACLE gave up at 256 frames)".to_string());
        }
        let result = self.eval_inner(form, env);
        self.depth -= 1;
        result
    }

    fn eval_inner(&mut self, form: Value, env: &Env) -> EvalResult {
        // `form` and `env` are rebound on tail calls instead of recursing.
        let mut form = form;
        let mut env = env.clone();

        loop {
            match form {
                Value::Symbol(ref name) => {
                    return env
                        .get(name)
                        .ok_or_else(|| format!("unbound name: {}", name))
                }
                Value::List(ref items) if !items.is_empty() => {
                    let head = items[0].clone();
                    let args = &items[1..];

                    if let Value::Symbol(ref name) = head {
                        match &**name {
                            "quote" => {
                                return args
                                    .first()
                                    .cloned()
                                    .ok_or_else(|| "quote needs one argument".to_string())
                            }
                            "if" => {
                                if args.len() < 2 {
                                    return Err("if needs a condition and a then-branch".to_string());
                                }
                                let test = self.eval(args[0].clone(), &env)?;
                                if test.is_truthy() {
                                    form = args[1].clone();
                                } else if let Some(alternative) = args.get(2) {
                                    form = alternative.clone();
                                } else {
                                    return Ok(Value::Nil);
                                }
                                continue;
                            }
                            "define" => return self.eval_define(args, &env),
                            "set!" => {
                                if args.len() != 2 {
                                    return Err("set! needs a name and a value".to_string());
                                }
                                let name = match &args[0] {
                                    Value::Symbol(name) => name.clone(),
                                    other => {
                                        return Err(format!(
                                            "set! needs a name, got {}",
                                            other.type_name()
                                        ))
                                    }
                                };
                                let value = self.eval(args[1].clone(), &env)?;
                                if env.set(&name, value.clone()) {
                                    return Ok(value);
                                }
                                return Err(format!("set!: {} is not bound", name));
                            }
                            "lambda" | "fn" => return self.eval_lambda(args, &env, "anonymous"),
                            "let" => {
                                let (body, scope) = self.eval_let(args, &env)?;
                                match body {
                                    Some(tail) => {
                                        form = tail;
                                        env = scope;
                                        continue;
                                    }
                                    None => return Ok(Value::Nil),
                                }
                            }
                            "begin" | "do" => {
                                if args.is_empty() {
                                    return Ok(Value::Nil);
                                }
                                for statement in &args[..args.len() - 1] {
                                    self.eval(statement.clone(), &env)?;
                                }
                                form = args[args.len() - 1].clone();
                                continue;
                            }
                            "while" => {
                                if args.is_empty() {
                                    return Err("while needs a condition".to_string());
                                }
                                let mut iterations = 0u64;
                                while self.eval(args[0].clone(), &env)?.is_truthy() {
                                    iterations += 1;
                                    if iterations > MAX_ITERATIONS {
                                        return Err(
                                            "while ran too long; ORACLE stopped it".to_string()
                                        );
                                    }
                                    for statement in &args[1..] {
                                        self.eval(statement.clone(), &env)?;
                                    }
                                }
                                return Ok(Value::Nil);
                            }
                            "cond" => {
                                let mut chosen = None;
                                for clause in args {
                                    let clause = clause.as_list()?;
                                    if clause.is_empty() {
                                        return Err("empty cond clause".to_string());
                                    }
                                    let is_else = matches!(
                                        &clause[0],
                                        Value::Symbol(name) if &**name == "else"
                                    );
                                    if is_else || self.eval(clause[0].clone(), &env)?.is_truthy() {
                                        chosen = Some(if clause.len() > 1 {
                                            Value::list(
                                                core::iter::once(Value::symbol("begin"))
                                                    .chain(clause[1..].iter().cloned())
                                                    .collect(),
                                            )
                                        } else {
                                            clause[0].clone()
                                        });
                                        break;
                                    }
                                }
                                match chosen {
                                    Some(tail) => {
                                        form = tail;
                                        continue;
                                    }
                                    None => return Ok(Value::Nil),
                                }
                            }
                            "and" => {
                                let mut last = Value::Bool(true);
                                for argument in args {
                                    last = self.eval(argument.clone(), &env)?;
                                    if !last.is_truthy() {
                                        return Ok(last);
                                    }
                                }
                                return Ok(last);
                            }
                            "or" => {
                                for argument in args {
                                    let value = self.eval(argument.clone(), &env)?;
                                    if value.is_truthy() {
                                        return Ok(value);
                                    }
                                }
                                return Ok(Value::Bool(false));
                            }
                            _ => {}
                        }
                    }

                    // Ordinary application.
                    let callee = self.eval(head, &env)?;
                    let mut evaluated = Vec::with_capacity(args.len());
                    for argument in args {
                        evaluated.push(self.eval(argument.clone(), &env)?);
                    }

                    match callee {
                        Value::Builtin(_, function) => return function(self, evaluated),
                        Value::Lambda(lambda) => {
                            let scope = bind_arguments(&lambda, evaluated)?;
                            if lambda.body.is_empty() {
                                return Ok(Value::Nil);
                            }
                            for statement in &lambda.body[..lambda.body.len() - 1] {
                                self.eval(statement.clone(), &scope)?;
                            }
                            // Tail call: loop rather than recurse.
                            form = lambda.body[lambda.body.len() - 1].clone();
                            env = scope;
                            continue;
                        }
                        other => {
                            return Err(format!("{} is not callable", other.type_name()));
                        }
                    }
                }
                // Empty list and every self-evaluating value.
                other => return Ok(other),
            }
        }
    }

    fn eval_define(&mut self, args: &[Value], env: &Env) -> EvalResult {
        if args.is_empty() {
            return Err("define needs a name".to_string());
        }
        match &args[0] {
            // (define name value)
            Value::Symbol(name) => {
                let value = match args.get(1) {
                    Some(form) => self.eval(form.clone(), env)?,
                    None => Value::Nil,
                };
                if let Value::Lambda(lambda) = &value {
                    *lambda.name.borrow_mut() = name.to_string();
                }
                env.define(name.clone(), value);
                Ok(Value::Symbol(name.clone()))
            }
            // (define (name args...) body...)
            Value::List(signature) => {
                let Some(Value::Symbol(name)) = signature.first() else {
                    return Err("define: expected a function name".to_string());
                };
                let mut lambda_args = Vec::with_capacity(args.len());
                lambda_args.push(Value::list(signature[1..].to_vec()));
                lambda_args.extend(args[1..].iter().cloned());
                let value = self.eval_lambda(&lambda_args, env, name)?;
                env.define(name.clone(), value);
                Ok(Value::Symbol(name.clone()))
            }
            other => Err(format!("define: cannot bind {}", other.type_name())),
        }
    }

    fn eval_lambda(&mut self, args: &[Value], env: &Env, name: &str) -> EvalResult {
        if args.is_empty() {
            return Err("lambda needs a parameter list".to_string());
        }
        let declared = args[0].as_list()?;
        let mut params = Vec::new();
        let mut rest = None;
        let mut collecting_rest = false;
        for item in declared {
            let Value::Symbol(symbol) = item else {
                return Err("lambda parameters must be names".to_string());
            };
            if &**symbol == "&rest" {
                collecting_rest = true;
                continue;
            }
            if collecting_rest {
                rest = Some(symbol.clone());
            } else {
                params.push(symbol.clone());
            }
        }
        Ok(Value::Lambda(Rc::new(Lambda {
            name: RefCell::new(name.to_string()),
            params,
            rest,
            body: args[1..].to_vec(),
            env: env.clone(),
        })))
    }

    /// Returns the tail expression to evaluate plus the scope to evaluate it in.
    fn eval_let(&mut self, args: &[Value], env: &Env) -> Result<(Option<Value>, Env), String> {
        if args.is_empty() {
            return Err("let needs a binding list".to_string());
        }
        let scope = Env::child(env);
        for binding in args[0].as_list()? {
            let pair = binding.as_list()?;
            let Some(Value::Symbol(name)) = pair.first() else {
                return Err("let bindings look like (name value)".to_string());
            };
            let value = match pair.get(1) {
                Some(form) => self.eval(form.clone(), &scope)?,
                None => Value::Nil,
            };
            scope.define(name.clone(), value);
        }
        if args.len() < 2 {
            return Ok((None, scope));
        }
        for statement in &args[1..args.len() - 1] {
            self.eval(statement.clone(), &scope)?;
        }
        Ok((Some(args[args.len() - 1].clone()), scope))
    }
}

fn bind_arguments(lambda: &Lambda, mut arguments: Vec<Value>) -> Result<Env, String> {
    let expected = lambda.params.len();
    if arguments.len() < expected || (lambda.rest.is_none() && arguments.len() > expected) {
        return Err(format!(
            "{} expects {} argument(s), got {}",
            lambda.name.borrow(),
            expected,
            arguments.len()
        ));
    }
    let scope = Env::child(&lambda.env);
    let extra = arguments.split_off(expected);
    for (name, value) in lambda.params.iter().zip(arguments) {
        scope.define(name.clone(), value);
    }
    if let Some(rest) = &lambda.rest {
        scope.define(rest.clone(), Value::list(extra));
    }
    Ok(scope)
}
