use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::num::{ParseFloatError, ParseIntError};
use std::process::Child;
use std::rc::Rc;

use crate::builtins_util::is_proper_list;
use crate::environment::*;
use crate::process::*;

#[derive(Clone, Debug)]
pub struct ParseError {
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct Lambda {
    pub params: Box<Expression>,
    pub body: Box<Expression>,
    pub capture: Rc<RefCell<Scope>>,
}

#[derive(Clone, Debug)]
pub struct Macro {
    pub params: Box<Expression>,
    pub body: Box<Expression>,
}

#[derive(Clone, Debug)]
pub enum Atom {
    Nil,
    True,
    Float(f64),
    Int(i64),
    Symbol(String),
    String(String),
    Lambda(Lambda),
    Macro(Macro),
}

impl Atom {
    pub fn to_string(&self) -> String {
        match self {
            Atom::Nil => "nil".to_string(),
            Atom::True => "true".to_string(),
            Atom::Float(f) => format!("{}", f),
            Atom::Int(i) => format!("{}", i),
            Atom::Symbol(s) => s.clone(),
            Atom::String(s) => s.clone(),
            Atom::Lambda(l) => {
                format!("Lambda ({}) ({})", l.params.to_string(), l.body.to_string())
            }
            Atom::Macro(m) => format!("Macro ({}) ({})", m.params.to_string(), m.body.to_string()),
        }
    }

    pub fn display_type(&self) -> String {
        match self {
            Atom::Nil => "Nil".to_string(),
            Atom::True => "True".to_string(),
            Atom::Float(_) => "Float".to_string(),
            Atom::Int(_) => "Int".to_string(),
            Atom::Symbol(_) => "Symbol".to_string(),
            Atom::String(_) => "String".to_string(),
            Atom::Lambda(_) => "Lambda".to_string(),
            Atom::Macro(_) => "Macro".to_string(),
        }
    }
}

#[derive(Clone, Copy)]
pub enum ProcessState {
    Running(u32),   // pid
    Over(u32, i32), // pid and exit status
}

#[derive(Clone)]
pub enum FileState {
    Stdin,
    Stdout,
    Stderr,
    Read(Rc<RefCell<BufReader<File>>>),
    Write(Rc<RefCell<BufWriter<File>>>),
    Closed,
}

#[derive(Clone)]
pub enum Expression {
    Atom(Atom),
    // RefCell the vector to allow destructive forms.
    List(Rc<RefCell<Vec<Expression>>>),
    Pair(Rc<RefCell<Expression>>, Rc<RefCell<Expression>>),
    Func(fn(&mut Environment, &[Expression]) -> io::Result<Expression>),
    Process(ProcessState),
    File(FileState),
}

impl fmt::Debug for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expression::Atom(a) => write!(f, "Expression::Atom({:?})", a),
            Expression::List(l) => write!(f, "Expression::List({:?})", l.borrow()),
            Expression::Pair(e1, e2) => {
                write!(f, "Expression::Pair({:?} . {:?})", e1.borrow(), e2.borrow())
            }
            Expression::Func(_) => write!(f, "Expression::Func(_)"),
            Expression::Process(ProcessState::Running(pid)) => {
                write!(f, "Expression::Process(ProcessStats::Running({}))", pid)
            }
            Expression::Process(ProcessState::Over(pid, exit_status)) => write!(
                f,
                "Expression::Process(ProcessState::Over({}, {}))",
                pid, exit_status
            ),
            Expression::File(_) => write!(f, "Expression::File(_)"),
        }
    }
}

impl Expression {
    pub fn with_list(list: Vec<Expression>) -> Expression {
        Expression::List(Rc::new(RefCell::new(list)))
    }

    pub fn to_string(&self) -> String {
        match self {
            Expression::Atom(a) => a.to_string(),
            Expression::Process(ProcessState::Running(pid)) => format!("{}", pid).to_string(),
            Expression::Process(ProcessState::Over(pid, _exit_status)) => {
                format!("{}", pid).to_string()
            }
            Expression::Func(_) => "Func".to_string(),
            Expression::List(list) => {
                let mut res = String::new();
                res.push_str("( ");
                for exp in list.borrow().iter() {
                    res.push_str(&exp.to_string());
                    res.push_str(" ");
                }
                res.push(')');
                res
            }
            Expression::Pair(e1, e2) => {
                if is_proper_list(self) {
                    let mut res = String::new();
                    res.push_str("#( ");
                    let mut current = e2.borrow().clone();
                    res.push_str(&e1.borrow().to_string());
                    res.push_str(" ");
                    while let Expression::Pair(e1, e2) = current {
                        res.push_str(&e1.borrow().to_string());
                        res.push_str(" ");
                        current = e2.borrow().clone();
                    }
                    res.push(')');
                    res
                } else {
                    format!(
                        "( {} . {} )",
                        e1.borrow().to_string(),
                        e2.borrow().to_string()
                    )
                }
            }
            Expression::File(_) => "File".to_string(),
        }
    }

    pub fn display_type(&self) -> String {
        match self {
            Expression::Atom(a) => a.display_type(),
            Expression::Process(_) => "Process".to_string(),
            Expression::Func(_) => "Func".to_string(),
            Expression::List(_) => "List".to_string(),
            Expression::Pair(_, _) => "Pair".to_string(),
            Expression::File(_) => "File".to_string(),
        }
    }

    fn pid_to_string(
        &self,
        procs: Rc<RefCell<HashMap<u32, Child>>>,
        pid: u32,
    ) -> io::Result<String> {
        match procs.borrow_mut().get_mut(&pid) {
            Some(child) => {
                if child.stdout.is_some() {
                    let mut buffer = String::new();
                    child.stdout.as_mut().unwrap().read_to_string(&mut buffer)?;
                    Ok(buffer)
                } else {
                    Ok("".to_string())
                }
            }
            None => Ok("".to_string()),
        }
    }

    pub fn make_string(&self, environment: &Environment) -> io::Result<String> {
        match self {
            Expression::Atom(a) => Ok(a.to_string()),
            Expression::Process(ProcessState::Running(_pid)) => Ok("".to_string()),
            Expression::Process(ProcessState::Over(pid, _exit_status)) => {
                self.pid_to_string(environment.procs.clone(), *pid)
            }
            Expression::Func(_) => Ok("".to_string()),
            Expression::List(list) => {
                let mut res = String::new();
                res.push_str("( ");
                for exp in list.borrow().iter() {
                    res.push_str(&exp.make_string(environment)?);
                    res.push_str(" ");
                }
                res.push(')');
                Ok(res)
            }
            Expression::Pair(e1, e2) => {
                if is_proper_list(self) {
                    let mut res = String::new();
                    res.push_str("#( ");
                    res.push_str(&e1.borrow().make_string(environment)?);
                    res.push_str(" ");
                    let mut current = e2.borrow().clone();
                    while let Expression::Pair(e1, e2) = current {
                        res.push_str(&e1.borrow().make_string(environment)?);
                        res.push_str(" ");
                        current = e2.borrow().clone();
                    }
                    res.push(')');
                    Ok(res)
                } else {
                    Ok(format!(
                        "( {} . {} )",
                        e1.borrow().make_string(environment)?,
                        e2.borrow().make_string(environment)?
                    ))
                }
            }
            Expression::File(FileState::Stdin) => {
                let f = io::stdin();
                let mut f = f.lock();
                let mut out_str = String::new();
                f.read_to_string(&mut out_str)?;
                Ok(out_str)
            }
            Expression::File(FileState::Stdout) => Ok("".to_string()), //  XXX error instead?
            Expression::File(FileState::Stderr) => Ok("".to_string()), //  XXX error instead?
            Expression::File(FileState::Read(file)) => {
                let mut f = file.borrow_mut();
                let mut out_str = String::new();
                f.read_to_string(&mut out_str)?;
                Ok(out_str)
            }
            Expression::File(FileState::Write(_)) => Ok("".to_string()), //  XXX error instead?
            Expression::File(FileState::Closed) => Ok("".to_string()),   //  XXX error instead?
        }
    }

    pub fn make_float(&self, environment: &Environment) -> io::Result<f64> {
        match self {
            Expression::Atom(Atom::Float(f)) => Ok(*f),
            Expression::Atom(Atom::Int(i)) => Ok(*i as f64),
            Expression::Atom(_) => Err(io::Error::new(io::ErrorKind::Other, "Not a number")),
            Expression::Process(ProcessState::Running(_pid)) => Err(io::Error::new(
                io::ErrorKind::Other,
                "Not a number (process still running!)",
            )),
            Expression::Process(ProcessState::Over(pid, _exit_status)) => {
                let buffer = self.pid_to_string(environment.procs.clone(), *pid)?;
                let potential_float: Result<f64, ParseFloatError> = buffer.parse();
                match potential_float {
                    Ok(v) => Ok(v),
                    Err(_) => Err(io::Error::new(io::ErrorKind::Other, "Not a number")),
                }
            }
            Expression::Func(_) => Err(io::Error::new(io::ErrorKind::Other, "Not a number")),
            Expression::List(_) => Err(io::Error::new(io::ErrorKind::Other, "Not a number")),
            Expression::Pair(_, _) => Err(io::Error::new(io::ErrorKind::Other, "Not a number")),
            Expression::File(_) => Err(io::Error::new(io::ErrorKind::Other, "Not a number")),
        }
    }

    pub fn make_int(&self, environment: &Environment) -> io::Result<i64> {
        match self {
            Expression::Atom(Atom::Int(i)) => Ok(*i),
            Expression::Atom(_) => Err(io::Error::new(io::ErrorKind::Other, "Not an integer")),
            Expression::Process(ProcessState::Running(_pid)) => Err(io::Error::new(
                io::ErrorKind::Other,
                "Not an integer (process still running!)",
            )),
            Expression::Process(ProcessState::Over(pid, _exit_status)) => {
                let buffer = self.pid_to_string(environment.procs.clone(), *pid)?;
                let potential_int: Result<i64, ParseIntError> = buffer.parse();
                match potential_int {
                    Ok(v) => Ok(v),
                    Err(_) => Err(io::Error::new(io::ErrorKind::Other, "Not an integer")),
                }
            }
            Expression::Func(_) => Err(io::Error::new(io::ErrorKind::Other, "Not an integer")),
            Expression::List(_) => Err(io::Error::new(io::ErrorKind::Other, "Not an integer")),
            Expression::Pair(_, _) => Err(io::Error::new(io::ErrorKind::Other, "Not an integer")),
            Expression::File(_) => Err(io::Error::new(io::ErrorKind::Other, "Not an integer")),
        }
    }

    pub fn writef(&self, environment: &Environment, writer: &mut dyn Write) -> io::Result<()> {
        match self {
            Expression::Atom(a) => write!(writer, "{}", a.to_string())?,
            Expression::Process(ps) => {
                let pid = match ps {
                    ProcessState::Running(pid) => pid,
                    ProcessState::Over(pid, _exit_status) => pid,
                };
                let procs = environment.procs.clone();
                let mut procs = procs.borrow_mut();
                match procs.get_mut(&pid) {
                    Some(child) => {
                        if child.stdout.is_some() {
                            let out = child.stdout.as_mut().unwrap();
                            let mut buf = [0; 1024];
                            loop {
                                match out.read(&mut buf) {
                                    Ok(0) => break,
                                    Ok(n) => writer.write_all(&buf[..n])?,
                                    Err(err) => return Err(err),
                                }
                            }
                        } else {
                            return Err(io::Error::new(
                                io::ErrorKind::Other,
                                "Failed to get process out to write to.",
                            ));
                        }
                    }
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "Failed to get process to write to.",
                        ));
                    }
                }
                drop(procs);
                wait_pid(environment, *pid, None);
            }
            Expression::Func(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    "Can not write a function",
                ))
            }
            Expression::List(list) => {
                write!(writer, "( ")?;
                for exp in list.borrow().iter() {
                    exp.writef(environment, writer)?;
                    write!(writer, " ")?;
                }
                write!(writer, ")")?;
            }
            Expression::Pair(e1, e2) => {
                if is_proper_list(self) {
                    write!(writer, "#( ")?;
                    e1.borrow().writef(environment, writer)?;
                    write!(writer, " ")?;
                    let mut current = e2.borrow().clone();
                    while let Expression::Pair(e1, e2) = current {
                        e1.borrow().writef(environment, writer)?;
                        write!(writer, " ")?;
                        current = e2.borrow().clone();
                    }
                    write!(writer, ")")?;
                } else {
                    write!(
                        writer,
                        "( {} . {} )",
                        e1.borrow().to_string(),
                        e2.borrow().to_string()
                    )?;
                }
            }
            Expression::File(FileState::Stdin) => {
                let f = io::stdin();
                let mut f = f.lock();
                let mut buf = [0; 1024];
                loop {
                    match f.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => writer.write_all(&buf[..n])?,
                        Err(err) => return Err(err),
                    }
                }
            }
            Expression::File(FileState::Read(file)) => {
                let mut f = file.borrow_mut();
                let mut buf = [0; 1024];
                loop {
                    match f.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => writer.write_all(&buf[..n])?,
                        Err(err) => return Err(err),
                    }
                }
            }
            Expression::File(_) => {
                return Err(io::Error::new(io::ErrorKind::Other, "Not a readable file."))
            }
        }
        writer.flush()?;
        Ok(())
    }

    pub fn write(&self, environment: &Environment) -> io::Result<()> {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        self.writef(environment, &mut handle)
    }
}
