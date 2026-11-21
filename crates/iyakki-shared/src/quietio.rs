//! Output that can never panic. `println!` and `eprintln!` panic when the write fails ("failed printing to stdout"): a console routed to a serial port
//! that does not exist, a closed pipe or a job whose reader went away all do that, and on the shell or PID 1 path a panic stops the machine's session.
//! Everything in this crate that prints from non-test code uses [`qprintln!`] and [`qeprintln!`], which ignore a failed write (the line was only for
//! a person who is no longer listening).

pub use iyakki_linuxinit::quietio::{err_line, line_to, out_line};

/// `println!` that ignores a failed write.
#[macro_export]
macro_rules! qprintln {
    () => { $crate::quietio::out_line(format_args!("")) };
    ($($t:tt)*) => { $crate::quietio::out_line(format_args!($($t)*)) };
}

/// `eprintln!` that ignores a failed write.
#[macro_export]
macro_rules! qeprintln {
    () => { $crate::quietio::err_line(format_args!("")) };
    ($($t:tt)*) => { $crate::quietio::err_line(format_args!($($t)*)) };
}
