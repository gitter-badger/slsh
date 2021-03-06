use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::io;
use std::process::Child;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::builtins::add_builtins;
use crate::builtins_file::add_file_builtins;
use crate::builtins_hashmap::add_hash_builtins;
use crate::builtins_io::add_io_builtins;
use crate::builtins_math::add_math_builtins;
use crate::builtins_pair::add_pair_builtins;
use crate::builtins_str::add_str_builtins;
use crate::builtins_types::add_type_builtins;
use crate::builtins_vector::add_vec_builtins;
use crate::process::*;
use crate::types::*;

#[derive(Clone, Debug)]
pub enum IOState {
    Pipe,
    Inherit,
    Null,
}

#[derive(Clone, Debug)]
pub struct EnvState {
    pub recur_num_args: Option<usize>,
    pub gensym_count: u32,
    pub stdout_status: Option<IOState>,
    pub stderr_status: Option<IOState>,
    pub eval_level: u32,
    pub is_spawn: bool,
    pub pipe_pgid: Option<u32>,
}

impl Default for EnvState {
    fn default() -> Self {
        EnvState {
            recur_num_args: None,
            gensym_count: 0,
            stdout_status: None,
            stderr_status: None,
            eval_level: 0,
            is_spawn: false,
            pipe_pgid: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormType {
    Any,
    FormOnly,
    ExternalOnly,
}

#[derive(Clone, Debug)]
pub struct Scope {
    pub data: HashMap<String, Rc<Expression>>,
    pub outer: Option<Rc<RefCell<Scope>>>,
    // If this scope is a namespace it will have a name otherwise it will be None.
    pub name: Option<String>,
}

impl Default for Scope {
    fn default() -> Self {
        let mut data = HashMap::new();
        add_builtins(&mut data);
        add_math_builtins(&mut data);
        add_str_builtins(&mut data);
        add_vec_builtins(&mut data);
        add_file_builtins(&mut data);
        add_io_builtins(&mut data);
        add_pair_builtins(&mut data);
        add_hash_builtins(&mut data);
        add_type_builtins(&mut data);
        data.insert(
            "*stdin*".to_string(),
            Rc::new(Expression::File(FileState::Stdin)),
        );
        data.insert(
            "*stdout*".to_string(),
            Rc::new(Expression::File(FileState::Stdout)),
        );
        data.insert(
            "*stderr*".to_string(),
            Rc::new(Expression::File(FileState::Stderr)),
        );
        data.insert(
            "*ns*".to_string(),
            Rc::new(Expression::Atom(Atom::String("root".to_string()))),
        );
        Scope {
            data,
            outer: None,
            name: Some("root".to_string()),
        }
    }
}

impl Scope {
    pub fn with_data<S: ::std::hash::BuildHasher>(
        environment: Option<&Environment>,
        mut data_in: HashMap<String, Rc<Expression>, S>,
    ) -> Scope {
        let mut data: HashMap<String, Rc<Expression>> = HashMap::with_capacity(data_in.len());
        for (k, v) in data_in.drain() {
            data.insert(k, v);
        }
        let outer = if let Some(environment) = environment {
            if let Some(scope) = environment.current_scope.last() {
                Some(scope.clone())
            } else {
                None
            }
        } else {
            None
        };
        Scope {
            data,
            outer,
            name: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum JobStatus {
    Running,
    Stopped,
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            JobStatus::Running => write!(f, "Running"),
            JobStatus::Stopped => write!(f, "Stopped"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    pub pids: Vec<u32>,
    pub names: Vec<String>,
    pub status: JobStatus,
}

#[derive(Clone, Debug)]
pub struct Environment {
    // Set to true when a SIGINT (ctrl-c) was received, lets long running stuff die.
    pub sig_int: Arc<AtomicBool>,
    pub state: EnvState,
    pub stopped_procs: Rc<RefCell<Vec<u32>>>,
    pub jobs: Rc<RefCell<Vec<Job>>>,
    pub in_pipe: bool,
    pub run_background: bool,
    pub is_tty: bool,
    pub do_job_control: bool,
    pub loose_symbols: bool,
    pub str_ignore_expand: bool,
    pub procs: Rc<RefCell<HashMap<u32, Child>>>,
    pub data_in: Option<Expression>,
    pub form_type: FormType,
    pub save_exit_status: bool,
    pub stack_on_error: bool,
    pub error_expression: Option<Expression>,
    // If this is Some then need to unwind and exit with then provided code (exit was called).
    pub exit_code: Option<i32>,
    // This is the dynamic bindings.  These take precidence over the other
    // bindings.
    pub dynamic_scope: HashMap<String, Rc<Expression>>,
    // This is the environment's root (global scope), it will also be part of
    // higher level scopes and in the current_scope vector (the first item).
    // It's special so keep a reference here as well for handy access.
    pub root_scope: Rc<RefCell<Scope>>,
    // Use as a stack of scopes, entering a new pushes and it gets popped on exit
    // The actual lookups are done using the scope and it's outer chain NOT this stack.
    pub current_scope: Vec<Rc<RefCell<Scope>>>,
    // Map of all the created namespaces.
    pub namespaces: HashMap<String, Rc<RefCell<Scope>>>,
}

pub fn build_default_environment(sig_int: Arc<AtomicBool>) -> Environment {
    let procs: Rc<RefCell<HashMap<u32, Child>>> = Rc::new(RefCell::new(HashMap::new()));
    let root_scope = Rc::new(RefCell::new(Scope::default()));
    let mut current_scope = Vec::new();
    current_scope.push(root_scope.clone());
    let mut namespaces = HashMap::new();
    namespaces.insert("root".to_string(), root_scope.clone());
    Environment {
        sig_int,
        state: EnvState::default(),
        stopped_procs: Rc::new(RefCell::new(Vec::new())),
        jobs: Rc::new(RefCell::new(Vec::new())),
        in_pipe: false,
        run_background: false,
        is_tty: true,
        do_job_control: true,
        loose_symbols: false,
        str_ignore_expand: false,
        procs,
        data_in: None,
        form_type: FormType::Any,
        save_exit_status: true,
        stack_on_error: false,
        error_expression: None,
        exit_code: None,
        dynamic_scope: HashMap::new(),
        root_scope,
        current_scope,
        namespaces,
    }
}

pub fn build_new_spawn_scope<S: ::std::hash::BuildHasher>(
    mut data_in: HashMap<String, Expression, S>,
    sig_int: Arc<AtomicBool>,
) -> Environment {
    let procs: Rc<RefCell<HashMap<u32, Child>>> = Rc::new(RefCell::new(HashMap::new()));
    let mut state = EnvState::default();
    let mut data: HashMap<String, Rc<Expression>> = HashMap::with_capacity(data_in.len());
    data.insert(
        "*ns*".to_string(),
        Rc::new(Expression::Atom(Atom::String("root".to_string()))),
    );
    for (k, v) in data_in.drain() {
        data.insert(k, Rc::new(v));
    }
    state.is_spawn = true;
    let root_scope = Rc::new(RefCell::new(Scope::with_data(None, data)));
    let mut current_scope = Vec::new();
    current_scope.push(root_scope.clone());
    let mut namespaces = HashMap::new();
    namespaces.insert("root".to_string(), root_scope.clone());
    Environment {
        sig_int,
        state,
        stopped_procs: Rc::new(RefCell::new(Vec::new())),
        jobs: Rc::new(RefCell::new(Vec::new())),
        in_pipe: false,
        run_background: false,
        is_tty: false,
        do_job_control: false,
        loose_symbols: false,
        str_ignore_expand: false,
        procs,
        data_in: None,
        form_type: FormType::Any,
        save_exit_status: true,
        stack_on_error: false,
        error_expression: None,
        exit_code: None,
        dynamic_scope: HashMap::new(),
        root_scope,
        current_scope,
        namespaces,
    }
}

pub fn build_new_scope(outer: Option<Rc<RefCell<Scope>>>) -> Rc<RefCell<Scope>> {
    let data: HashMap<String, Rc<Expression>> = HashMap::new();
    Rc::new(RefCell::new(Scope {
        data,
        outer,
        name: None,
    }))
}

pub fn build_new_namespace(
    environment: &mut Environment,
    name: &str,
) -> Result<Rc<RefCell<Scope>>, String> {
    if environment.namespaces.contains_key(name) {
        let msg = format!("Namespace {} already exists!", name);
        Err(msg)
    } else {
        let mut data: HashMap<String, Rc<Expression>> = HashMap::new();
        data.insert(
            "*ns*".to_string(),
            Rc::new(Expression::Atom(Atom::String(name.to_string()))),
        );
        let scope = Scope {
            data,
            outer: Some(environment.root_scope.clone()),
            name: Some(name.to_string()),
        };
        let scope = Rc::new(RefCell::new(scope));
        environment
            .namespaces
            .insert(name.to_string(), scope.clone());
        Ok(scope)
    }
}

pub fn clone_symbols<S: ::std::hash::BuildHasher>(
    scope: &Scope,
    data_in: &mut HashMap<String, Expression, S>,
) {
    for (k, v) in &scope.data {
        let v = &**v;
        data_in.insert(k.clone(), v.clone());
    }
    if let Some(outer) = &scope.outer {
        clone_symbols(&outer.borrow(), data_in);
    }
}

pub fn get_expression(environment: &Environment, key: &str) -> Option<Rc<Expression>> {
    if environment.dynamic_scope.contains_key(key) {
        Some(environment.dynamic_scope.get(key).unwrap().clone())
    } else if key.contains("::") {
        // namespace reference.
        let mut key_i = key.splitn(2, "::");
        if let Some(namespace) = key_i.next() {
            if let Some(scope) = environment.namespaces.get(namespace) {
                if let Some(key) = key_i.next() {
                    if let Some(exp) = scope.borrow().data.get(key) {
                        return Some(exp.clone());
                    }
                }
            }
        }
        None
    } else {
        let mut loop_scope = Some(environment.current_scope.last().unwrap().clone());
        while let Some(scope) = loop_scope {
            if let Some(exp) = scope.borrow().data.get(key) {
                return Some(exp.clone());
            }
            loop_scope = scope.borrow().outer.clone();
        }
        None
    }
}

pub fn overwrite_expression(environment: &mut Environment, key: &str, expression: Rc<Expression>) {
    if environment.dynamic_scope.contains_key(key) {
        environment
            .dynamic_scope
            .insert(key.to_string(), expression);
    } else if key.contains("::") {
        // namespace reference.
        let mut key_i = key.splitn(2, "::");
        if let Some(namespace) = key_i.next() {
            if let Some(scope) = environment.namespaces.get(namespace) {
                if let Some(key) = key_i.next() {
                    if scope.borrow().data.contains_key(key) {
                        scope.borrow_mut().data.insert(key.to_string(), expression);
                    }
                }
            }
        }
    } else {
        let mut loop_scope = Some(environment.current_scope.last().unwrap().clone());
        while let Some(scope) = loop_scope {
            if scope.borrow().data.contains_key(key) {
                scope.borrow_mut().data.insert(key.to_string(), expression);
                return;
            }
            loop_scope = scope.borrow().outer.clone();
        }
    }
}

pub fn set_expression_current(
    environment: &mut Environment,
    key: String,
    expression: Rc<Expression>,
) {
    environment
        .current_scope
        .last()
        .unwrap() // Always has at least root scope unless horribly broken.
        .borrow_mut()
        .data
        .insert(key, expression);
}

pub fn remove_expression_current(environment: &mut Environment, key: &str) {
    environment
        .current_scope
        .last()
        .unwrap() // Always has at least root scope unless horribly broken.
        .borrow_mut()
        .data
        .remove(key);
}

pub fn is_expression(environment: &Environment, key: &str) -> bool {
    if key.starts_with('$') {
        env::var(&key[1..]).is_ok()
    } else {
        get_expression(environment, key).is_some()
    }
}

pub fn get_symbols_scope(environment: &Environment, key: &str) -> Option<Rc<RefCell<Scope>>> {
    // DO NOT return a namespace for a namespace key otherwise set will work to
    // set symbols in other namespaces.
    if !key.contains("::") {
        let mut loop_scope = Some(environment.current_scope.last().unwrap().clone());
        while loop_scope.is_some() {
            let scope = loop_scope.unwrap();
            if let Some(_exp) = scope.borrow().data.get(key) {
                return Some(scope.clone());
            }
            loop_scope = scope.borrow().outer.clone();
        }
    }
    None
}

pub fn get_namespace(environment: &Environment, name: &str) -> Option<Rc<RefCell<Scope>>> {
    if environment.namespaces.contains_key(name) {
        Some(environment.namespaces.get(name).unwrap().clone())
    } else {
        None
    }
}

pub fn mark_job_stopped(environment: &Environment, pid: u32) {
    'outer: for mut j in environment.jobs.borrow_mut().iter_mut() {
        for p in &j.pids {
            if *p == pid {
                j.status = JobStatus::Stopped;
                break 'outer;
            }
        }
    }
}

pub fn mark_job_running(environment: &Environment, pid: u32) {
    'outer: for mut j in environment.jobs.borrow_mut().iter_mut() {
        for p in &j.pids {
            if *p == pid {
                j.status = JobStatus::Running;
                break 'outer;
            }
        }
    }
}

pub fn remove_job(environment: &Environment, pid: u32) {
    let mut idx: Option<usize> = None;
    'outer: for (i, j) in environment.jobs.borrow_mut().iter_mut().enumerate() {
        for p in &j.pids {
            if *p == pid {
                idx = Some(i);
                break 'outer;
            }
        }
    }
    if let Some(i) = idx {
        environment.jobs.borrow_mut().remove(i);
    }
}

pub fn add_process(environment: &Environment, process: Child) -> u32 {
    let pid = process.id();
    environment.procs.borrow_mut().insert(pid, process);
    pid
}

pub fn reap_procs(environment: &Environment) -> io::Result<()> {
    let mut procs = environment.procs.borrow_mut();
    let keys: Vec<u32> = procs.keys().copied().collect();
    let mut pids: Vec<u32> = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(child) = procs.get_mut(&key) {
            pids.push(child.id());
        }
    }
    drop(procs);
    for pid in pids {
        try_wait_pid(environment, pid);
    }
    // XXX remove them or better replace pid with exit status
    Ok(())
}
