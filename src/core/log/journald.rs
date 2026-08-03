use std::{
	cell::RefCell,
	env::args_os,
	ffi::OsStr,
	io::{self, Write, stderr},
	os::unix::net::UnixDatagram,
	path::Path,
};

use tracing::{Level, Metadata};

use super::is_systemd_mode;
use crate::{
	Config, Result,
	arrayvec::ArrayString,
	err, implement,
	smallstr::SmallString,
	smallvec::SmallVec,
	utils::{
		math::{ExpectInto, Expected},
		string::to_array_string,
	},
};

#[cfg(test)]
mod tests;

/// Encoded journal fields; a longer run spills to the heap.
type Buffer = SmallVec<[u8; 128]>;

/// Syslog identifier tagging every entry.
type Identifier = SmallString<[u8; 16]>;

/// Formatted source line number.
type CodeLine = ArrayString<10>;

const SOCKET: &str = "/run/systemd/journal/socket";

const IDENTIFIER: &str = "tuwunel";

/// Bounds the datagram well under the default socket send buffer.
const PAYLOAD_MAX: usize = 128 * 1024;

const LEN_PREFIX: usize = size_of::<u64>();

/// Datagram connection to the journald submission socket.
pub struct Journal {
	socket: UnixDatagram,
	identifier: Identifier,
}

/// One entry, submitted when the writer is dropped. Its payload accumulates
/// in the thread's buffer.
pub struct Entry<'a> {
	journal: &'a Journal,
	message: usize,
}

thread_local! {
	/// Entry under construction on this thread, reused across events.
	static PAYLOAD: RefCell<Buffer> = const { RefCell::new(Buffer::new_const()) };
}

impl Journal {
	/// Opens the socket when journald is configured, keeping the console when
	/// nothing is listening on it.
	#[must_use]
	pub fn open(config: &Config) -> Option<Self> {
		enabled(config)
			.then(Self::new)?
			.inspect_err(|e| {
				writeln!(stderr(), "{e}").ok();
			})
			.ok()
	}

	fn new() -> Result<Self> {
		let journal = Self {
			socket: UnixDatagram::unbound()?,
			identifier: identifier(),
		};

		journal
			.send(&[])
			.map_err(|e| err!(Config("log_journald", "{SOCKET}: {e}.")))?;

		Ok(journal)
	}
}

/// Tags entries with the running executable's name.
fn identifier() -> Identifier {
	args_os()
		.next()
		.as_deref()
		.map(Path::new)
		.and_then(Path::file_name)
		.and_then(OsStr::to_str)
		.map_or_else(|| IDENTIFIER.into(), Into::into)
}

/// Opens an entry at the event's severity, carrying its source metadata, and
/// leaves the message field open.
#[implement(Journal)]
#[must_use]
pub fn entry(&self, meta: &Metadata<'_>) -> Entry<'_> {
	let message = PAYLOAD.with_borrow_mut(|payload| {
		put(payload, "PRIORITY", &[priority(*meta.level())]);
		put(payload, "SYSLOG_IDENTIFIER", self.identifier.as_bytes());
		put(payload, "TARGET", meta.target().as_bytes());

		if let Some(file) = meta.file() {
			put(payload, "CODE_FILE", file.as_bytes());
		}

		if let Some(line) = meta.line() {
			let line: CodeLine = to_array_string(line);

			put(payload, "CODE_LINE", line.as_bytes());
		}

		payload.extend_from_slice(b"MESSAGE\n");
		payload.extend_from_slice(&[0; LEN_PREFIX]);

		payload.len()
	});

	Entry { journal: self, message }
}

#[implement(Journal)]
fn send(&self, payload: &[u8]) -> io::Result<usize> { self.socket.send_to(payload, SOCKET) }

impl Write for Entry<'_> {
	fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
		PAYLOAD.with_borrow_mut(|payload| payload.extend_from_slice(buf));

		Ok(buf.len())
	}

	fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

impl Drop for Entry<'_> {
	fn drop(&mut self) {
		PAYLOAD.with_borrow_mut(|payload| {
			close(payload, self.message);

			if self.journal.send(payload).is_err() {
				fallback(&payload[self.message..]);
			}

			payload.clear();
		});
	}
}

/// Writes a message the socket would not take to the console, which journald
/// captures in turn under systemd.
fn fallback(message: &[u8]) { stderr().write_all(message).ok(); }

/// Seals the message field, trimming the formatter's trailing whitespace and
/// truncating on a character boundary what the datagram cannot carry.
#[expect(
	clippy::little_endian_bytes,
	reason = "the journal protocol specifies little-endian field lengths"
)]
fn close(payload: &mut Buffer, message: usize) {
	let budget = PAYLOAD_MAX.saturating_sub(message);
	let len = payload[message..].trim_ascii_end().len();
	let len = (len > budget)
		.then(|| boundary(&payload[message..], budget))
		.unwrap_or(len);

	let size: u64 = len.expect_into();

	payload.truncate(message.expected_add(len));
	payload[message.expected_sub(LEN_PREFIX)..message].copy_from_slice(&size.to_le_bytes());
	payload.push(b'\n');
}

/// Steps back to the nearest character boundary at or below `len`; a
/// continuation byte never begins a character.
fn boundary(message: &[u8], len: usize) -> usize {
	(0..=len)
		.rev()
		.find(|&i| {
			message
				.get(i)
				.is_none_or(|byte| byte & 0b1100_0000 != 0b1000_0000)
		})
		.unwrap_or_default()
}

/// Whether events are submitted to journald rather than written to the
/// console, which journald otherwise captures at a single fixed priority.
fn enabled(config: &Config) -> bool { config.log_journald && is_systemd_mode() }

/// Appends a length-encoded field, which may carry any byte sequence.
#[expect(
	clippy::little_endian_bytes,
	reason = "the journal protocol specifies little-endian field lengths"
)]
fn put(payload: &mut Buffer, name: &str, value: &[u8]) {
	let len: u64 = value.len().expect_into();

	payload.extend_from_slice(name.as_bytes());
	payload.push(b'\n');
	payload.extend_from_slice(&len.to_le_bytes());
	payload.extend_from_slice(value);
	payload.push(b'\n');
}

/// Maps a level onto the syslog severity code journald expects.
const fn priority(level: Level) -> u8 {
	match level {
		| Level::ERROR => b'3',
		| Level::WARN => b'4',
		| Level::INFO => b'5',
		| Level::DEBUG => b'6',
		| Level::TRACE => b'7',
	}
}
