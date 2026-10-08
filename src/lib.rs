//! Buffered input and output that flushes output before reading from the source.
//!
//! Synchronous [`Read`] cannot report ahead of time whether a read will block.
//! Consequently, output is flushed whenever the input buffer is empty, before
//! calling the underlying reader, even if that read immediately returns.

use std::io::{self, BufRead, BufReader, BufWriter, IoSlice, IoSliceMut, Read, Write};
use std::{cell::RefCell, rc::Rc};

type Output<'a> = Rc<RefCell<dyn Write + 'a>>;
type Outputs<'a> = Rc<RefCell<Vec<Output<'a>>>>;

/// A group of any number of buffered inputs and outputs.
///
/// Before any input reads from its source, all registered outputs are flushed
/// in registration order. Buffered input reads do not flush outputs.
/// Handles and cloned groups share the same registry, including outputs added
/// after an input was created. This group is intended for single-threaded use.
///
/// The registry retains outputs even after their writer handles are dropped.
/// Call `flush` to handle final output errors; destruction ignores such errors
/// just as `BufWriter` does. Separate groups operate independently.
#[derive(Clone, Default)]
pub struct BufferedIoGroup<'a> {
    outputs: Outputs<'a>,
}

impl<'a> BufferedIoGroup<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an input whose source reads flush this group's outputs.
    pub fn reader<R: Read>(&self, reader: R) -> BufReader<FlushingReader<'a, R>> {
        BufReader::new(FlushingReader {
            reader,
            group: self.clone(),
        })
    }

    pub fn reader_with_capacity<R: Read>(
        &self,
        capacity: usize,
        reader: R,
    ) -> BufReader<FlushingReader<'a, R>> {
        BufReader::with_capacity(
            capacity,
            FlushingReader {
                reader,
                group: self.clone(),
            },
        )
    }

    /// Registers an output, retaining its buffer until the group is released.
    pub fn writer<W: Write + 'a>(&self, writer: W) -> GroupWriter<W> {
        self.register(BufWriter::new(writer))
    }

    pub fn writer_with_capacity<W: Write + 'a>(
        &self,
        capacity: usize,
        writer: W,
    ) -> GroupWriter<W> {
        self.register(BufWriter::with_capacity(capacity, writer))
    }

    fn register<W: Write + 'a>(&self, writer: BufWriter<W>) -> GroupWriter<W> {
        let writer = Rc::new(RefCell::new(writer));
        self.outputs.borrow_mut().push(writer.clone());
        GroupWriter { writer }
    }

    /// Attempts to flush every output, returning the first error if any fail.
    /// Successful flushes cannot be rolled back when another output fails.
    pub fn flush(&self) -> io::Result<()> {
        let outputs = self.outputs.borrow().clone();
        let mut first_error = None;
        for output in outputs {
            let result = match output.try_borrow_mut() {
                Ok(mut writer) => writer.flush(),
                Err(_) => Err(io::Error::other("output is already in use")),
            };
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// The source adapter used inside a group's `BufReader`.
/// Wrapping the source ensures all buffer refill and direct-read paths flush.
pub struct FlushingReader<'a, R> {
    reader: R,
    group: BufferedIoGroup<'a>,
}

impl<R: Read> Read for FlushingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.group.flush()?;
        self.reader.read(buf)
    }

    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        if bufs.iter().all(|buf| buf.is_empty()) {
            return Ok(0);
        }
        self.group.flush()?;
        self.reader.read_vectored(bufs)
    }
}

/// A handle to one buffered output in a group.
/// Its explicit `flush` flushes only this output; use the group to flush all.
pub struct GroupWriter<W: Write> {
    writer: Rc<RefCell<BufWriter<W>>>,
}

impl<W: Write> Write for GroupWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.borrow_mut().write(buf)
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        self.writer.borrow_mut().write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.borrow_mut().flush()
    }
}

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

    fn group_reader(events: &Events, data: &[u8]) -> Reader {
        Reader {
            data: Cursor::new(data.to_vec()),
            events: events.clone(),
        }
    }

    fn group_writer(events: &Events, fail_flush: bool) -> Writer {
        Writer {
            events: events.clone(),
            fail_flush,
        }
    }

    #[test]
    fn either_input_flushes_every_output_and_preserves_buffered_reads() -> io::Result<()> {
        let events = Events::default();
        let group = BufferedIoGroup::new();
        let mut first = group.reader_with_capacity(4, group_reader(&events, b"abcdef"));
        let mut second = group.reader(group_reader(&events, b"xyz"));
        // Registration after creating inputs must also be visible to them.
        let mut out1 = group.writer(group_writer(&events, false));
        let mut out2 = group.writer(group_writer(&events, false));
        out1.write_all(b"1")?;
        out2.write_all(b"2")?;
        first.read_exact(&mut [0])?;
        assert_eq!(
            *events.borrow(),
            vec![
                Event::Write(b"1".to_vec()),
                Event::Flush,
                Event::Write(b"2".to_vec()),
                Event::Flush,
                Event::Read,
            ]
        );
        events.borrow_mut().clear();
        out1.write_all(b"3")?;
        out2.write_all(b"4")?;
        first.read_exact(&mut [0])?;
        assert!(events.borrow().is_empty());
        second.read_exact(&mut [0])?;
        assert_eq!(
            *events.borrow(),
            vec![
                Event::Write(b"3".to_vec()),
                Event::Flush,
                Event::Write(b"4".to_vec()),
                Event::Flush,
                Event::Read,
            ]
        );
        events.borrow_mut().clear();
        out1.write_all(b"5")?;
        out2.write_all(b"6")?;
        let mut crossing = [0; 3];
        first.read_exact(&mut crossing)?;
        assert_eq!(&crossing, b"cde");
        assert_eq!(
            *events.borrow(),
            vec![
                Event::Write(b"5".to_vec()),
                Event::Flush,
                Event::Write(b"6".to_vec()),
                Event::Flush,
                Event::Read,
            ]
        );
        Ok(())
    }

    #[test]
    fn group_flush_error_still_flushes_other_outputs_and_blocks_input() -> io::Result<()> {
        for operation in 0..3 {
            let events = Events::default();
            let group = BufferedIoGroup::new();
            let mut input = group.reader_with_capacity(4, group_reader(&events, b"abcdef"));
            let mut bad = group.writer(group_writer(&events, true));
            let mut good = group.writer(group_writer(&events, false));
            bad.write_all(b"1")?;
            good.write_all(b"2")?;
            let mut buf = [0; 8];
            let error = match operation {
                0 => input.read(&mut buf).unwrap_err(),
                1 => input
                    .read_vectored(&mut [IoSliceMut::new(&mut buf)])
                    .unwrap_err(),
                _ => input.fill_buf().unwrap_err(),
            };
            assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
            assert_eq!(
                *events.borrow(),
                vec![
                    Event::Write(b"1".to_vec()),
                    Event::Flush,
                    Event::Write(b"2".to_vec()),
                    Event::Flush,
                ]
            );
        }
        Ok(())
    }

    #[test]
    fn group_source_adapter_covers_read_line_read_to_end_and_direct_reads() -> io::Result<()> {
        for operation in 0..4 {
            let events = Events::default();
            let group = BufferedIoGroup::new();
            let mut input = group.reader_with_capacity(4, group_reader(&events, b"abcdef\n"));
            let mut out = group.writer(group_writer(&events, false));
            out.write_all(b">")?;
            let mut buf = [0; 16];
            match operation {
                0 => {
                    assert_eq!(input.read(&mut buf)?, 7);
                }
                1 => {
                    assert_eq!(input.read_vectored(&mut [IoSliceMut::new(&mut buf)])?, 7);
                }
                2 => {
                    let mut line = String::new();
                    input.read_line(&mut line)?;
                    assert_eq!(line, "abcdef\n");
                }
                _ => {
                    let mut data = Vec::new();
                    input.read_to_end(&mut data)?;
                    assert_eq!(data, b"abcdef\n");
                }
            }
            let events = events.borrow();
            assert_eq!(
                &events[..3],
                &[Event::Write(b">".to_vec()), Event::Flush, Event::Read]
            );
            for (index, event) in events.iter().enumerate() {
                if *event == Event::Read {
                    assert_eq!(events[index - 1], Event::Flush);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn group_retains_dropped_outputs_and_handles_outlive_group() -> io::Result<()> {
        let events = Events::default();
        let group = BufferedIoGroup::new();
        let mut input = group.reader(group_reader(&events, b"abc"));
        let mut output = group.writer(group_writer(&events, false));
        output.write_all(b">")?;
        drop(output);
        drop(group);
        assert!(events.borrow().is_empty());
        input.read_exact(&mut [0])?;
        assert_eq!(
            *events.borrow(),
            vec![Event::Write(b">".to_vec()), Event::Flush, Event::Read]
        );
        Ok(())
    }

    #[test]
    fn groups_are_independent_and_allow_different_output_types() -> io::Result<()> {
        let events = Events::default();
        let group = BufferedIoGroup::new();
        let other = BufferedIoGroup::new();
        let mut input = group.reader(group_reader(&events, b"abc"));
        let mut output = other.writer(group_writer(&events, false));
        let mut memory = group.writer(Vec::<u8>::new());
        memory.write_all(b"memory")?;
        output.write_all(b">")?;
        input.read_exact(&mut [0])?;
        assert_eq!(*events.borrow(), vec![Event::Read]);
        events.borrow_mut().clear();
        other.flush()?;
        assert_eq!(
            *events.borrow(),
            vec![Event::Write(b">".to_vec()), Event::Flush]
        );
        Ok(())
    }
}
