# buffered-io

`BufReader` と `BufWriter` を組み合わせた同期 I/O。
入力バッファが空で、読み込み元を呼ぶ直前に出力をすべて flush します。
入力バッファにデータが残っている間は、出力をバッファに保持します。

```rust
use buffered_io::BufferedIo;
use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut io = BufferedIo::new(stdin.lock(), stdout.lock());

    write!(io, "名前: ")?;
    let mut name = String::new();
    io.read_line(&mut name)?; // 読み込み元を呼ぶ前にプロンプトを flush
    writeln!(io, "こんにちは、{}さん", name.trim_end())?;
    io.flush() // 終了時の flush エラーも処理
}
```

`Read`・`BufRead`・`Write` を実装しているので、`read_exact`、`read_line`、
`write_all`、`write!` などを利用できます。依存クレートはありません。
`with_capacity` で入出力のバッファ容量を個別に指定できます。

同期の `Read` には「次の読み込みが実際に入力待ちになるか」を事前判定する
仕組みがありません。そのため、即座に読める場合や EOF の場合にも、
入力バッファが空なら flush します。実際に待つ場合だけ flush するには、
読み込み元に応じた非ブロッキング I/O や readiness 判定が必要です。
`stdin` 自体にある内部バッファなど、ラップした読み込み元の内部状態は判定しません。

flush に失敗した場合は、そのエラーを返し、読み込み元は呼びません。
空のスライスへの `read` / `read_vectored` は flush も読み込みもしません。
`into_parts` は両方のバッファを保持したまま分離します。

```sh
cargo run
cargo test
```
