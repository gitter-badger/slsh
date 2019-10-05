use liner::Context;
use std::cell::RefCell;
use std::env;
use std::ffi::CStr;
use std::fs;
use std::fs::create_dir_all;
use std::io::{self, ErrorKind};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use nix::sys::signal::{self, SigHandler, Signal};
use nix::unistd::gethostname;

use crate::completions::*;
use crate::environment::*;
use crate::eval::*;
use crate::reader::*;
use crate::types::*;

fn load_scripts(environment: &mut Environment, home: &str) {
    let mut script = format!("{}/.config/slsh/slsh_std.lisp", home);
    if let Err(err) = run_script(&script, environment) {
        eprintln!(
            "WARNING: Failed to load standard macros script {}: {}",
            script, err
        );
    }
    script = format!("{}/.config/slsh/slsh_shell.lisp", home);
    if let Err(err) = run_script(&script, environment) {
        eprintln!(
            "WARNING: Failed to load shell macros script {}: {}",
            script, err
        );
    }
    script = format!("{}/.config/slsh/slshrc", home);
    if let Err(err) = run_script(&script, environment) {
        eprintln!("WARNING: Failed to load init script {}: {}", script, err);
    }
}

fn get_prompt(environment: &mut Environment) -> String {
    if environment
        .root_scope
        .borrow()
        .data
        .contains_key("__prompt")
    {
        let mut exp = environment
            .root_scope
            .borrow()
            .data
            .get("__prompt")
            .unwrap()
            .clone();
        exp = match *exp {
            Expression::Atom(Atom::Lambda(_)) => {
                let mut v = Vec::with_capacity(1);
                v.push(Expression::Atom(Atom::Symbol("__prompt".to_string())));
                Rc::new(Expression::with_list(v))
            }
            _ => exp,
        };
        environment.save_exit_status = false; // Do not overwrite last exit status with prompt commands.
        let res = eval(environment, &exp);
        environment.save_exit_status = true;
        res.unwrap_or_else(|e| Expression::Atom(Atom::String(format!("ERROR: {}", e).to_string())))
            .make_string(environment)
            .unwrap_or_else(|_| "ERROR".to_string())
    } else {
        // Nothing set, use a default.
        let hostname = match env::var("HOST") {
            Ok(val) => val,
            Err(_) => "UNKNOWN".to_string(),
        };
        let pwd = match env::current_dir() {
            Ok(val) => val,
            Err(_) => {
                let mut p = PathBuf::new();
                p.push("/");
                p
            }
        };
        format!(
            "\x1b[32m{}:\x1b[34m{}\x1b[37m(slsh)\x1b[32m>\x1b[39m ",
            hostname,
            pwd.display()
        )
    }
}

pub fn start_interactive(sig_int: Arc<AtomicBool>) {
    let mut con = Context::new();
    con.history.append_duplicate_entries = false;
    con.history.inc_append = true;
    con.history.load_duplicates = false;
    con.history.share = true;
    con.key_bindings = liner::KeyBindings::Vi;
    // Initialize the HOST variable
    let mut hostname = [0_u8; 512];
    env::set_var(
        "HOST",
        &gethostname(&mut hostname)
            .ok()
            .map_or_else(|| "?".into(), CStr::to_string_lossy)
            .as_ref(),
    );
    let mut home = match env::var("HOME") {
        Ok(val) => val,
        Err(_) => ".".to_string(),
    };
    if home.ends_with('/') {
        home = home[..home.len() - 1].to_string();
    }
    let share_dir = format!("{}/.local/share/slsh", home);
    if let Err(err) = create_dir_all(&share_dir) {
        eprintln!(
            "WARNING: Unable to create share directory: {}- {}",
            share_dir, err
        );
    }
    if let Err(err) = con
        .history
        .set_file_name_and_load_history(format!("{}/history", share_dir))
    {
        eprintln!("WARNING: Unable to load history: {}", err);
    }
    let environment = Rc::new(RefCell::new(build_default_environment(sig_int)));
    load_scripts(&mut environment.borrow_mut(), &home);
    environment
        .borrow_mut()
        .root_scope
        .borrow_mut()
        .data
        .insert(
            "*last-status*".to_string(),
            Rc::new(Expression::Atom(Atom::Int(0))),
        );
    loop {
        environment.borrow_mut().state.stdout_status = None;
        environment.borrow_mut().state.stderr_status = None;
        // Clear the SIGINT if one occured.
        environment
            .borrow()
            .sig_int
            .compare_and_swap(true, false, Ordering::Relaxed);
        let prompt = get_prompt(&mut environment.borrow_mut());
        if let Err(err) = reap_procs(&environment.borrow()) {
            eprintln!("Error reaping processes: {}", err);
        }
        let mut shell_completer = ShellCompleter::new(environment.clone());
        match con.read_line(prompt, None, &mut shell_completer) {
            Ok(input) => {
                if input.is_empty() {
                    continue;
                }
                let mod_input = if input.starts_with('(')
                    || input.starts_with('\'')
                    || input.starts_with('`')
                {
                    input.clone()
                } else {
                    format!("({})", input)
                };
                // Clear the last status once something new is entered.
                env::set_var("LAST_STATUS".to_string(), format!("{}", 0));
                environment
                    .borrow_mut()
                    .root_scope
                    .borrow_mut()
                    .data
                    .insert(
                        "*last-status*".to_string(),
                        Rc::new(Expression::Atom(Atom::Int(i64::from(0)))),
                    );
                let ast = read(&mod_input);
                match ast {
                    Ok(ast) => {
                        environment.borrow_mut().loose_symbols = true;
                        let res = eval(&mut environment.borrow_mut(), &ast);
                        match res {
                            Ok(exp) => {
                                if !input.is_empty() {
                                    if let Err(err) = con.history.push(input.into()) {
                                        eprintln!("Error saving history: {}", err);
                                    }
                                }
                                match exp {
                                    Expression::Atom(Atom::Nil) => { /* don't print nil */ }
                                    Expression::Process(_) => { /* should have used stdout */ }
                                    _ => {
                                        if let Err(err) = exp.write(&environment.borrow()) {
                                            eprintln!("Error writing result: {}", err);
                                        }
                                    }
                                }
                            }
                            Err(err) => eprintln!("{}", err),
                        }
                        environment.borrow_mut().loose_symbols = false;
                    }
                    Err(err) => eprintln!("{:?}", err),
                }
            }
            Err(err) => match err.kind() {
                ErrorKind::UnexpectedEof => return,
                ErrorKind::Interrupted => {}
                _ => println!("Error on input: {}", err),
            },
        }
    }
}

pub fn read_stdin() {
    let mut home = match env::var("HOME") {
        Ok(val) => val,
        Err(_) => ".".to_string(),
    };
    if home.ends_with('/') {
        home = home[..home.len() - 1].to_string();
    }
    let share_dir = format!("{}/.local/share/slsh", home);
    if let Err(err) = create_dir_all(&share_dir) {
        eprintln!(
            "WARNING: Unable to create share directory: {}- {}",
            share_dir, err
        );
    }
    let mut environment = build_default_environment(Arc::new(AtomicBool::new(false)));
    environment.is_tty = false;
    load_scripts(&mut environment, &home);

    let mut input = String::new();
    loop {
        match io::stdin().read_line(&mut input) {
            Ok(0) => return,
            Ok(_n) => {
                environment.state.stdout_status = None;
                let mod_input = if input.starts_with('(')
                    || input.starts_with('\'')
                    || input.starts_with('`')
                {
                    input.clone()
                } else {
                    format!("({})", input)
                };
                let ast = read(&mod_input);
                match ast {
                    Ok(ast) => {
                        environment.loose_symbols = true;
                        match eval(&mut environment, &ast) {
                            Ok(exp) => {
                                match exp {
                                    Expression::Atom(Atom::Nil) => { /* don't print nil */ }
                                    Expression::Process(_) => { /* should have used stdout */ }
                                    _ => {
                                        if let Err(err) = exp.write(&environment) {
                                            eprintln!("Error writing result: {}", err);
                                        }
                                    }
                                }
                            }
                            Err(err) => eprintln!("{}", err),
                        }
                        environment.loose_symbols = false;
                    }
                    Err(err) => eprintln!("{:?}", err),
                }
                environment.state.stderr_status = None;
            }
            Err(error) => {
                eprintln!("ERROR reading stdin: {}", error);
                return;
            }
        }
    }
}

fn parse_one_run_command_line(input: &str, nargs: &mut Vec<String>) -> io::Result<()> {
    let mut in_string = false;
    let mut in_stringd = false;
    let mut token = String::new();
    let mut last_ch = ' ';
    for ch in input.chars() {
        if ch == '\'' && last_ch != '\\' {
            // Kakoune bug "
            in_string = !in_string;
            if !in_string {
                nargs.push(token);
                token = String::new();
            }
            last_ch = ch;
            continue;
        }
        if ch == '"' && last_ch != '\\' {
            // Kakoune bug "
            in_stringd = !in_stringd;
            if !in_stringd {
                nargs.push(token);
                token = String::new();
            }
            last_ch = ch;
            continue;
        }
        if in_string || in_stringd {
            token.push(ch);
        } else if ch == ' ' {
            if !token.is_empty() {
                nargs.push(token);
                token = String::new();
            }
        } else {
            token.push(ch);
        }
        last_ch = ch;
    }
    if !token.is_empty() {
        nargs.push(token);
    }
    Ok(())
}

pub fn run_one_command(command: &str, args: &[String]) -> io::Result<()> {
    // Try to make sense out of whatever crap we get (looking at you fzf-tmux)
    // and make it work.
    let mut nargs: Vec<String> = Vec::new();
    parse_one_run_command_line(command, &mut nargs)?;
    for arg in args {
        parse_one_run_command_line(&arg, &mut nargs)?;
    }

    if !nargs.is_empty() {
        let mut com = Command::new(&nargs[0]);
        if nargs.len() > 1 {
            com.args(&nargs[1..]);
        }
        com.stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdin(Stdio::inherit());

        unsafe {
            com.pre_exec(|| -> io::Result<()> {
                signal::signal(Signal::SIGINT, SigHandler::SigDfl).unwrap();
                signal::signal(Signal::SIGHUP, SigHandler::SigDfl).unwrap();
                signal::signal(Signal::SIGTERM, SigHandler::SigDfl).unwrap();
                Ok(())
            });
        }

        let mut proc = com.spawn()?;
        proc.wait()?;
    }
    Ok(())
}

fn run_script(file_name: &str, environment: &mut Environment) -> io::Result<()> {
    let contents = fs::read_to_string(file_name)?;
    let ast = read(&contents);
    match ast {
        Ok(Expression::List(list)) => {
            for exp in list.borrow().iter() {
                match eval(environment, &exp) {
                    Ok(_exp) => {}
                    Err(err) => {
                        eprintln!("{}", err);
                        return Err(err);
                    }
                }
            }
            Ok(())
        }
        Ok(ast) => match eval(environment, &ast) {
            Ok(_exp) => Ok(()),
            Err(err) => {
                eprintln!("{}", err);
                Err(err)
            }
        },
        Err(err) => {
            eprintln!("{:?}", err);
            Err(io::Error::new(io::ErrorKind::Other, err.reason))
        }
    }
}

pub fn run_one_script(command: &str, args: &[String]) -> io::Result<()> {
    let mut environment = build_default_environment(Arc::new(AtomicBool::new(false)));
    let mut exp_args: Vec<Expression> = Vec::with_capacity(args.len());
    for a in args {
        exp_args.push(Expression::Atom(Atom::String(a.clone())));
    }
    environment
        .root_scope
        .borrow_mut()
        .data
        .insert("args".to_string(), Rc::new(Expression::with_list(exp_args)));
    run_script(command, &mut environment)
}
