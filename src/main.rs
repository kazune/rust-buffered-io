use buffered_io::BufferedIoGroup;
use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let stderr = io::stderr();
    let group = BufferedIoGroup::new();
    let mut input = group.reader(stdin.lock());
    let mut output = group.writer(stdout.lock());
    let mut diagnostics = group.writer(stderr.lock());

    write!(output, "名前: ")?;
    write!(diagnostics, "名前を入力してください。")?;
    let mut name = String::new();
    if input.read_line(&mut name)? != 0 {
        writeln!(output, "こんにちは、{}さん", name.trim_end())?;
    }
    group.flush()
}
