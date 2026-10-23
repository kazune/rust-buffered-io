# buffered-io

任意個数の入力と出力を `BufferedIoGroup` に登録する同期 I/O。
どれかの入力が読み込み元を呼ぶ直前に、同じグループの全出力を flush します。
入力バッファ内だけで読み込みが済む間は、出力をバッファに保持します。

```rust
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
    input.read_line(&mut name)?; // stdout と stderr の両方を flush してから読む
    writeln!(output, "こんにちは、{}さん", name.trim_end())?;
    group.flush() // 終了時も全出力を flush し、エラーを処理
}
```

`reader()` は通常の `BufReader` を返し、`writer()` のハンドルは `Write` を
実装しています。`read_exact`、`read_line`、`write_all`、`write!` などを
利用できます。依存クレートはありません。`reader_with_capacity()` と
`writer_with_capacity()` でバッファ容量を個別に指定できます。
異なる型の出力先も、同じグループに登録できます。

入力を作成した後に追加した出力も flush の対象です。
グループの clone は登録先を共有し、別の `new()` で作ったグループは独立します。
単一スレッド向けであり、`Send` / `Sync` には対応していません。

同期の `Read` には「次の読み込みが実際に入力待ちになるか」を事前判定する
仕組みがありません。そのため、即座に読める場合や EOF の場合にも、
入力バッファが空なら flush します。実際に待つ場合だけ flush するには、
読み込み元に応じた非ブロッキング I/O や readiness 判定が必要です。
`stdin` 自体にある内部バッファなど、ラップした読み込み元の内部状態は判定しません。

全出力を登録順に flush します。一つが失敗しても残りを試み、最初のエラーを
返して読み込み元は呼びません。すでに成功した flush や、入力バッファから
消費したデータは巻き戻しません。個別の出力ハンドルの `flush()` は、その出力だけを
flush します。バッファ補充だけでなく、大きな直接読み込みも全出力を flush します。
独自の `Write` 実装から同じ出力への書き込みや flush に再入した場合は、
借用競合による panic の代わりに `io::Error`（`ErrorKind::Other`）を返します。

出力ハンドルを drop しても、グループはその出力とバッファを保持します。
入力ハンドルもグループの登録先を保持するため、グループ変数を drop しても動作します。
最後のグループ・入力ハンドルを drop すると、登録先による出力の保持が終了します。
終了時のエラーを処理するには、その前に `group.flush()` を呼んでください。

グループから外す場合は、ハンドルを保持したまま `detach()` を呼びます。

```rust
output.detach();          // この出力は全出力 flush の対象から外れる
input.get_mut().detach(); // この入力は読み込み前にグループを flush しなくなる

// バッファとハンドルはそのまま使える
output.write_all(b"message")?;
output.flush()?;          // 外した出力は個別に flush する
```

解除そのものは読み込み・書き込み・flush を行いません。
未読の入力バッファと未出力の出力バッファは保持されます。
出力の解除は、全入力と clone したグループに反映されます。
入力の解除は、その入力だけに反映されます。繰り返し解除しても問題ありません。
解除した出力はグループが保持しなくなるため、最後の出力ハンドルを drop すると
通常の `BufWriter` と同様に残りの出力を試みますが、その際のエラーは無視されます。

1入力・1出力の場合も、同じグループに入力と出力を1つずつ登録します。

```sh
cargo run
cargo test
```
