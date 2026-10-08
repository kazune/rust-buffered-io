use buffered_io::BufferedIo;
use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut io = BufferedIo::new(stdin.lock(), stdout.lock());

    write!(io, "名前: ")?;
    let mut name = String::new();
    if io.read_line(&mut name)? != 0 {
        writeln!(io, "こんにちは、{}さん", name.trim_end())?;
    }
    io.flush()
}
