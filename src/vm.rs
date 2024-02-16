// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

use crate::Result;
use std::cell::RefCell;
use std::cmp;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Int(i32),
    Float(f32),
}

pub type Stack = Vec<Op>;
pub type Mailboxes = Rc<RefCell<HashMap<i32, Op>>>;

pub trait InstExtension {
    /// Returns the jump offset from the current operation.
    fn eval(&self, stack: &mut Stack, mboxes: &Mailboxes) -> Result<isize>;
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result;
}

impl fmt::Debug for dyn InstExtension {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.fmt(f)
    }
}

#[derive(Debug)]
pub enum Inst {
    /// Extensions are additional instructions not implemented here, but require dynamic dispatch
    /// which may be slower than the builtin instructions.
    Extension(Box<dyn InstExtension>),

    Add,
    Div,
    Dup,
    Hlt,
    Jmp,
    Jmplt,
    Jmpz,
    Mul,
    Nop,
    Pop,
    Push(Op),
    Peek,
    Recv,
    Send,
    Sub,
    Yield,
}

impl Inst {
    /// Returns the jump offset from the current operation.
    fn eval(&self, stack: &mut Stack, mboxes: &Mailboxes) -> Result<isize> {
        match self {
            Inst::Extension(e) => e.eval(stack, mboxes),
            Inst::Add => {
                if stack.len() < 2 {
                    Err("add requires 2 operands")
                } else {
                    let o1 = stack.pop().unwrap();
                    let o2 = stack.pop().unwrap();
                    let ret = match (o1, o2) {
                        (Op::Int(i1), Op::Int(i2)) => Op::Int(i1 + i2),
                        (Op::Float(f1), Op::Float(f2)) => Op::Float(f1 + f2),
                        (Op::Int(i1), Op::Float(f2)) => Op::Float(i1 as f32 + f2),
                        (Op::Float(f1), Op::Int(i2)) => Op::Float(f1 + i2 as f32),
                    };
                    stack.push(ret);
                    Ok(1)
                }
            }
            Inst::Div => {
                if stack.len() < 2 {
                    Err("div requires 2 operands")
                } else {
                    let o2 = stack.pop().unwrap();
                    let o1 = stack.pop().unwrap();
                    let ret = match (o1, o2) {
                        (Op::Int(i1), Op::Int(i2)) => {
                            if i2 == 0 {
                                return Err("divide by 0");
                            }
                            Op::Int(i1 / i2)
                        }
                        (Op::Float(f1), Op::Float(f2)) => {
                            if f2 == 0.0 {
                                return Err("divide by 0");
                            }
                            Op::Float(f1 / f2)
                        }
                        (Op::Int(i1), Op::Float(f2)) => {
                            if f2 == 0.0 {
                                return Err("divide by 0");
                            }
                            Op::Float(i1 as f32 / f2)
                        }
                        (Op::Float(f1), Op::Int(i2)) => {
                            if i2 == 0 {
                                return Err("divide by 0");
                            }
                            Op::Float(f1 / i2 as f32)
                        }
                    };
                    stack.push(ret);
                    Ok(1)
                }
            }
            Inst::Dup => {
                if stack.is_empty() {
                    Err("dup requires 1 operand")
                } else {
                    stack.push(stack.last().unwrap().clone());
                    Ok(1)
                }
            }
            Inst::Hlt => Ok(0),
            Inst::Jmp => {
                if stack.is_empty() {
                    Err("jmp requires 1 operand")
                } else {
                    let offset = stack.pop().unwrap();
                    match offset {
                        Op::Int(i) => Ok(i as isize),
                        Op::Float(f) => Ok(f as isize),
                    }
                }
            }
            Inst::Jmplt => {
                if stack.len() < 3 {
                    Err("jmplt requires 3 operands")
                } else {
                    let o1 = stack.pop().unwrap();
                    let o2 = stack.pop().unwrap();
                    let o3 = stack.pop().unwrap();
                    let offset = match o1 {
                        Op::Int(i) => i as isize,
                        Op::Float(f) => f as isize,
                    };
                    let lt = match (o2, o3) {
                        (Op::Int(i1), Op::Int(i2)) => i1 < i2,
                        (Op::Float(f1), Op::Float(f2)) => f1 < f2,
                        (Op::Int(i1), Op::Float(f2)) => (i1 as f32) < f2,
                        (Op::Float(f1), Op::Int(i2)) => f1 < (i2 as f32),
                    };
                    if lt {
                        Ok(offset)
                    } else {
                        Ok(1)
                    }
                }
            }
            Inst::Jmpz => {
                if stack.len() < 2 {
                    Err("jmpz requires 2 operands")
                } else {
                    let o1 = stack.pop().unwrap();
                    let o2 = stack.pop().unwrap();
                    let offset = match o1 {
                        Op::Int(i) => i as isize,
                        Op::Float(f) => f as isize,
                    };
                    match o2 {
                        Op::Int(0) => Ok(offset),
                        Op::Float(f) if f == 0.0 => Ok(offset),
                        _ => Ok(1),
                    }
                }
            }
            Inst::Mul => {
                if stack.len() < 2 {
                    Err("mul requires 2 operands")
                } else {
                    let o1 = stack.pop().unwrap();
                    let o2 = stack.pop().unwrap();
                    let ret = match (o1, o2) {
                        (Op::Int(i1), Op::Int(i2)) => Op::Int(i1 * i2),
                        (Op::Float(f1), Op::Float(f2)) => Op::Float(f1 * f2),
                        (Op::Int(i1), Op::Float(f2)) => Op::Float(i1 as f32 * f2),
                        (Op::Float(f1), Op::Int(i2)) => Op::Float(f1 * i2 as f32),
                    };
                    stack.push(ret);
                    Ok(1)
                }
            }
            Inst::Nop => Ok(1),
            Inst::Pop => {
                // If the stack is empty, pop is a nop.
                stack.pop();
                Ok(1)
            }
            Inst::Push(o) => {
                stack.push(o.clone());
                Ok(1)
            }
            Inst::Peek => {
                if stack.is_empty() {
                    Err("peek requires 1 operand")
                } else {
                    let mbox = match stack.pop().unwrap() {
                        Op::Int(i) => i,
                        Op::Float(f) => f as i32,
                    };
                    let map = mboxes.borrow();
                    if let Some(v) = map.get(&mbox) {
                        stack.push(v.clone());
                    }
                    Ok(1)
                }
            }
            Inst::Recv => {
                if stack.is_empty() {
                    Err("recv requires 1 operand")
                } else {
                    let mbox = match stack.pop().unwrap() {
                        Op::Int(i) => i,
                        Op::Float(f) => f as i32,
                    };
                    let mut map = mboxes.borrow_mut();
                    if let Some(v) = map.remove(&mbox) {
                        stack.push(v);
                    }
                    Ok(1)
                }
            }
            Inst::Send => {
                if stack.len() < 2 {
                    Err("send requires 2 operands")
                } else {
                    let op = stack.pop().unwrap();
                    let mbox = match stack.pop().unwrap() {
                        Op::Int(i) => i,
                        Op::Float(f) => f as i32,
                    };
                    let mut map = mboxes.borrow_mut();
                    map.insert(mbox, op);
                    Ok(1)
                }
            }
            Inst::Sub => {
                if stack.len() < 2 {
                    Err("sub requires 2 operands")
                } else {
                    let o2 = stack.pop().unwrap();
                    let o1 = stack.pop().unwrap();
                    let ret = match (o1, o2) {
                        (Op::Int(i1), Op::Int(i2)) => Op::Int(i1 - i2),
                        (Op::Float(f1), Op::Float(f2)) => Op::Float(f1 - f2),
                        (Op::Int(i1), Op::Float(f2)) => Op::Float(i1 as f32 - f2),
                        (Op::Float(f1), Op::Int(i2)) => Op::Float(f1 - i2 as f32),
                    };
                    stack.push(ret);
                    Ok(1)
                }
            }
            Inst::Yield => Ok(1),
        }
    }
}

pub type Program = Vec<Inst>;

impl From<&str> for Inst {
    fn from(s: &str) -> Self {
        match s {
            "add" => Inst::Add,
            "div" => Inst::Div,
            "dup" => Inst::Dup,
            "hlt" => Inst::Hlt,
            "jmp" => Inst::Jmp,
            "jmplt" => Inst::Jmplt,
            "jmpz" => Inst::Jmpz,
            "mul" => Inst::Mul,
            "nop" => Inst::Nop,
            "pop" => Inst::Pop,
            "peek" => Inst::Peek,
            "recv" => Inst::Recv,
            "send" => Inst::Send,
            "sub" => Inst::Sub,
            "yield" => Inst::Yield,
            _ => {
                if s.starts_with("push") {
                    let mut parts = s.split_whitespace();
                    parts.next();
                    let num = parts.next().expect("push requires argument");
                    Inst::Push(if num.contains('.') {
                        Op::Float(num.parse().unwrap())
                    } else {
                        Op::Int(num.parse().unwrap())
                    })
                } else {
                    panic!("unknown instruction: {}", s);
                }
            }
        }
    }
}

pub struct Core {
    pub prog: Program,
    pub mboxes: Mailboxes,
    pub stack: Stack,
    pub cycles: u64,
    pub pc: usize,
    pub errors: u64,
}

impl Core {
    pub fn new(prog: Program, mboxes: Mailboxes) -> Self {
        Self {
            prog,
            mboxes,
            stack: Stack::new(),
            cycles: 0,
            pc: 0,
            errors: 0,
        }
    }

    pub fn reset(&mut self) {
        self.stack.clear();
        self.cycles = 0;
        self.pc = 0;
        self.errors = 0;
        // Program was never mutated, and mailboxes is a shared resource that may also be used to
        // communicate results with the outside world.
    }

    /// Return the last error if one occurred during excution.
    pub fn eval(&mut self, cycles_delta: Option<u64>) -> Result<()> {
        let cycles_limit = match cycles_delta {
            Some(c) => self.cycles + c,
            None => u64::MAX,
        };
        let mut ret = Ok(());

        while self.is_running() && self.cycles < cycles_limit {
            let inst = &self.prog[self.pc];
            match inst.eval(&mut self.stack, &self.mboxes) {
                Ok(jmp) => {
                    let next_pc = (self.pc as isize) + jmp;
                    self.pc = cmp::max(next_pc, 0) as usize;
                }
                Err(e) => {
                    ret = Err(e);
                    self.pc += 1;
                    self.errors += 1;
                }
            }
            self.cycles += 1;

            if let Inst::Yield = inst {
                break;
            }
        }

        ret
    }

    pub fn is_running(&self) -> bool {
        self.pc < self.prog.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_instr() {
        match Inst::from("nop") {
            Inst::Nop => (),
            _ => panic!(),
        }
        match Inst::from("yield") {
            Inst::Yield => (),
            _ => panic!(),
        }
        match Inst::from("push 123") {
            Inst::Push(Op::Int(123)) => (),
            _ => panic!(),
        }
        match Inst::from("push 12.3") {
            Inst::Push(Op::Float(f)) if f == 12.3 => (),
            _ => panic!(),
        }
    }

    #[test]
    fn empty_prog() {
        let mut core = Core::new(vec![], Mailboxes::default());
        assert_eq!(core.eval(None), Ok(()));
        assert_eq!(core.cycles, 0);
        assert_eq!(core.pc, 0);
        assert!(core.stack.is_empty());
        assert!(!core.is_running());
    }

    #[test]
    fn nop_and_hlt() {
        let prog = vec![Inst::Nop, Inst::Nop, Inst::Hlt];
        let mut core = Core::new(prog, Mailboxes::default());
        assert_eq!(core.eval(Some(1)), Ok(()));
        assert_eq!(core.cycles, 1);
        assert_eq!(core.pc, 1);
        assert_eq!(core.eval(Some(98)), Ok(()));
        assert_eq!(core.cycles, 99);
        assert_eq!(core.pc, 2);
        assert!(core.is_running());
    }

    #[test]
    fn prog_with_err() {
        let prog = vec![
            Inst::Push(Op::Int(42)),
            Inst::Push(Op::Int(0)),
            Inst::Div,
            Inst::Push(Op::Int(11)),
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        assert_eq!(core.eval(None), Err("divide by 0"));
        assert_eq!(core.cycles, 4);
        assert_eq!(core.pc, 4);
        assert_eq!(core.stack, vec![Op::Int(11)]);
        assert!(!core.is_running());
    }

    #[test]
    fn send_and_recv() {
        let prog = vec![
            Inst::Push(Op::Int(111)),
            Inst::Recv,               // nothing received
            Inst::Push(Op::Int(111)), // dst
            Inst::Push(Op::Int(42)),  // msg
            Inst::Send,
            Inst::Push(Op::Int(111)), // dst
            Inst::Recv,
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        core.eval(Some(6)).unwrap();
        assert_eq!(core.cycles, 6);
        assert_eq!(core.pc, 6);
        assert!(core.is_running());
        assert_eq!(core.stack, vec![Op::Int(111)]);
        assert_eq!(core.mboxes.borrow().get(&111), Some(&Op::Int(42)));
        core.eval(None).unwrap();
        assert_eq!(core.cycles, 7);
        assert_eq!(core.pc, 7);
        assert!(!core.is_running());
        assert_eq!(core.stack, vec![Op::Int(42)]);
        assert_eq!(core.mboxes.borrow().get(&111), None);
    }

    #[test]
    fn jmp() {
        let prog = vec![
            Inst::Nop,
            Inst::Nop,
            Inst::Nop,
            Inst::Push(Op::Int(-4)),
            Inst::Jmp,
            Inst::Nop,
            Inst::Nop,
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        core.eval(Some(5)).unwrap();
        assert_eq!(core.cycles, 5);
        assert_eq!(core.pc, 0);
        assert_eq!(core.stack, vec![]);
        assert!(core.is_running());
    }

    #[test]
    fn jmpz_loop() {
        let prog = vec![
            Inst::Push(Op::Int(1)),
            Inst::Push(Op::Int(-1)),
            Inst::Add,
            Inst::Dup,
            Inst::Push(Op::Int(-4)),
            Inst::Jmpz,
            Inst::Nop,
            Inst::Nop,
        ];
        let mut core = Core::new(prog, Mailboxes::default());
        core.eval(None).unwrap();
        assert_eq!(core.cycles, 13);
        assert_eq!(core.pc, 8);
        assert_eq!(core.stack, vec![Op::Int(-1)]);
        assert!(!core.is_running());
    }

    #[test]
    fn yield_inst() {
        let prog = vec![Inst::Yield, Inst::Nop, Inst::Yield];
        let mut core = Core::new(prog, Mailboxes::default());
        core.eval(None).unwrap();
        assert!(core.is_running());
        assert_eq!(core.cycles, 1);
        assert_eq!(core.pc, 1);
        core.eval(None).unwrap();
        assert_eq!(core.cycles, 3);
        assert_eq!(core.pc, 3);
        assert!(!core.is_running());
    }
}
