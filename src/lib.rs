//! Buffered input and output that flushes output before reading from the source.
//!
//! Synchronous [`Read`] cannot report ahead of time whether a read will block.
//! Consequently, output is flushed whenever the input buffer is empty, before
//! calling the underlying reader, even if that read immediately returns.

use std::io::{self, BufRead, BufReader, BufWriter, IoSlice, IoSliceMut, Read, Write};

/// A coupled `BufReader` and `BufWriter` for interactive I/O.
///
/// Reads satisfied by the input buffer leave output buffered. Reads requiring
/// the underlying source first flush output, propagating any flush error without
/// reading input. As with `BufWriter`, explicitly call `flush` before dropping
/// this value if you need to handle errors while flushing the final output.
pub struct BufferedIo<R, W: Write> {
    reader: BufReader<R>,
    writer: BufWriter<W>,
}

impl<R: Read, W: Write> BufferedIo<R, W> {
    /// Uses the standard library's default buffer capacities.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer: BufWriter::new(writer),
        }
    }

    /// Sets the input and output buffer capacities independently, in bytes.
    pub fn with_capacity(
        read_capacity: usize,
        write_capacity: usize,
        reader: R,
        writer: W,
    ) -> Self {
        Self {
            reader: BufReader::with_capacity(read_capacity, reader),
            writer: BufWriter::with_capacity(write_capacity, writer),
        }
    }

    /// Separates the buffered reader and writer, preserving both buffers.
    ///
    /// This does not flush output; automatic flushing before input ends here.
    pub fn into_parts(self) -> (BufReader<R>, BufWriter<W>) {
        (self.reader, self.writer)
    }

    fn flush_before_read(&mut self) -> io::Result<()> {
        if self.reader.buffer().is_empty() {
            self.writer.flush()?;
        }
        Ok(())
    }
}

impl<R: Read, W: Write> Read for BufferedIo<R, W> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.flush_before_read()?;
        self.reader.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        if bufs.iter().all(|buf| buf.is_empty()) {
            return Ok(0);
        }
        self.flush_before_read()?;
        self.reader.read_vectored(bufs)
    }
}

impl<R: Read, W: Write> BufRead for BufferedIo<R, W> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.flush_before_read()?;
        self.reader.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount);
    }
}

impl<R, W: Write> Write for BufferedIo<R, W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.writer.write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, io::Cursor, rc::Rc};

    #[derive(Debug, PartialEq)]
    enum Event {
        Read,
        Write(Vec<u8>),
        Flush,
    }

    type Events = Rc<RefCell<Vec<Event>>>;

    struct Reader {
        data: Cursor<Vec<u8>>,
        events: Events,
    }

    impl Read for Reader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.events.borrow_mut().push(Event::Read);
            self.data.read(buf)
        }
    }

    struct Writer {
        events: Events,
        fail_flush: bool,
    }

    impl Write for Writer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            // Exercise BufWriter's handling of partial writes as well.
            let n = buf.len().min(2);
            self.events
                .borrow_mut()
                .push(Event::Write(buf[..n].to_vec()));
            Ok(n)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.events.borrow_mut().push(Event::Flush);
            if self.fail_flush {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "flush failed"))
            } else {
                Ok(())
            }
        }
    }

    fn fixture(data: &[u8], fail_flush: bool) -> (BufferedIo<Reader, Writer>, Events) {
        let events = Events::default();
        let reader = Reader {
            data: Cursor::new(data.to_vec()),
            events: events.clone(),
        };
        let writer = Writer {
            events: events.clone(),
            fail_flush,
        };
        (BufferedIo::with_capacity(4, 32, reader, writer), events)
    }

    #[test]
    fn flushes_all_output_before_source_read() -> io::Result<()> {
        let (mut io, events) = fixture(b"abcdef", false);
        io.write_all(b"hello")?;
        assert!(events.borrow().is_empty());
        let mut byte = [0];
        io.read_exact(&mut byte)?;
        assert_eq!(byte, *b"a");
        assert_eq!(
            *events.borrow(),
            vec![
                Event::Write(b"he".to_vec()),
                Event::Write(b"ll".to_vec()),
                Event::Write(b"o".to_vec()),
                Event::Flush,
                Event::Read,
            ]
        );

        events.borrow_mut().clear();
        io.write_all(b"next")?;
        let mut buffered = [0; 3];
        io.read_exact(&mut buffered)?;
        assert_eq!(&buffered, b"bcd");
        assert!(events.borrow().is_empty());
        io.read_exact(&mut byte)?;
        assert_eq!(byte, *b"e");
        assert_eq!(
            *events.borrow(),
            vec![
                Event::Write(b"ne".to_vec()),
                Event::Write(b"xt".to_vec()),
                Event::Flush,
                Event::Read,
            ]
        );
        Ok(())
    }

    #[test]
    fn line_reads_flush_at_each_refill() -> io::Result<()> {
        let (mut io, events) = fixture(b"abcde\n", false);
        io.write_all(b">")?;
        let mut line = String::new();
        assert_eq!(io.read_line(&mut line)?, 6);
        assert_eq!(line, "abcde\n");
        assert_eq!(
            *events.borrow(),
            vec![
                Event::Write(b">".to_vec()),
                Event::Flush,
                Event::Read,
                Event::Flush,
                Event::Read,
            ]
        );
        Ok(())
    }

    #[test]
    fn large_and_vectored_reads_flush_before_bypassing_buffer() -> io::Result<()> {
        for vectored in [false, true] {
            let (mut io, events) = fixture(b"abcdef", false);
            io.write_all(b">")?;
            let mut buf = [0; 8];
            let n = if vectored {
                io.read_vectored(&mut [IoSliceMut::new(&mut buf)])?
            } else {
                io.read(&mut buf)?
            };
            assert_eq!(&buf[..n], b"abcdef");
            assert_eq!(
                *events.borrow(),
                vec![Event::Write(b">".to_vec()), Event::Flush, Event::Read,]
            );
        }
        Ok(())
    }

    #[test]
    fn flush_error_prevents_source_read() -> io::Result<()> {
        for operation in 0..3 {
            let (mut io, events) = fixture(b"abc", true);
            io.write_all(b">")?;
            let mut buf = [0; 8];
            let error = match operation {
                0 => io.read(&mut buf).unwrap_err(),
                1 => io
                    .read_vectored(&mut [IoSliceMut::new(&mut buf)])
                    .unwrap_err(),
                _ => io.fill_buf().unwrap_err(),
            };
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert_eq!(
                *events.borrow(),
                vec![Event::Write(b">".to_vec()), Event::Flush]
            );
        }
        Ok(())
    }

    #[test]
    fn empty_reads_do_not_flush_or_read() -> io::Result<()> {
        let (mut io, events) = fixture(b"abc", true);
        io.write_all(b">")?;
        assert_eq!(io.read(&mut [])?, 0);
        assert_eq!(io.read_vectored(&mut [])?, 0);
        assert_eq!(io.read_vectored(&mut [IoSliceMut::new(&mut [])])?, 0);
        assert!(events.borrow().is_empty());
        Ok(())
    }

    #[test]
    fn eof_and_explicit_flush_work() -> io::Result<()> {
        let (mut io, events) = fixture(b"", false);
        io.write_all(b">")?;
        assert_eq!(io.read(&mut [0])?, 0);
        assert_eq!(
            *events.borrow(),
            vec![Event::Write(b">".to_vec()), Event::Flush, Event::Read]
        );
        events.borrow_mut().clear();
        io.write_all(b"!")?;
        io.flush()?;
        assert_eq!(
            *events.borrow(),
            vec![Event::Write(b"!".to_vec()), Event::Flush]
        );
        Ok(())
    }
}
