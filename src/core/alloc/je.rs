//! jemalloc allocator

use std::{
	alloc::Layout,
	cell::OnceCell,
	ffi::{CStr, c_char, c_void},
	fmt::Debug,
	panic::catch_unwind,
	process::abort,
	sync::{
		Mutex,
		atomic::{AtomicBool, AtomicU64, Ordering},
	},
};

use jevmalloc as jemalloc;
use jevmalloc::{
	ctl as mallctl, ffi,
	hook::{ALLOC, ALLOC_ZEROED},
};

use crate::{
	Result,
	arrayvec::ArrayVec,
	err, is_equal_to, is_nonzero,
	utils::{BoolExt, math, math::Tried},
};

/// Provides the process-wide jemalloc startup configuration.
///
/// Jemalloc reads this unmangled symbol during allocator initialization, which
/// can occur before `main`. The NUL-terminated options enable CPU-affine
/// arenas, background purging, metadata huge pages, and tuned cache and decay
/// thresholds.
#[cfg(feature = "jemalloc_conf")]
#[used]
#[unsafe(no_mangle)]
pub static malloc_conf: &[u8] = const_str::concat_bytes!(
	"tcache:true",
	",percpu_arena:percpu",
	",metadata_thp:always",
	",background_thread:true",
	",max_background_threads:-1",
	",lg_extent_max_active_fit:4",
	",oversize_threshold:2097152",
	",tcache_max:524288",
	",dirty_decay_ms:16000",
	",muzzy_decay_ms:144000",
	//MALLOC_CONF_PROF,
	0
);

#[cfg(all(
	feature = "jemalloc_conf",
	feature = "jemalloc_prof",
	target_arch = "x86_64",
))]
const _MALLOC_CONF_PROF: &str = ",prof_active:false";
#[cfg(all(
	feature = "jemalloc_conf",
	any(not(feature = "jemalloc_prof"), not(target_arch = "x86_64")),
))]
const _MALLOC_CONF_PROF: &str = "";

type Name = ArrayVec<u8, NAME_MAX>;
type Key = ArrayVec<usize, KEY_SEGS>;

const NAME_MAX: usize = 128;
const KEY_SEGS: usize = 8;

#[global_allocator]
static JEMALLOC: jemalloc::Jemalloc = jemalloc::Jemalloc;
static CONTROL: Mutex<()> = Mutex::new(());

static GLOBAL_ALLOCS: AtomicU64 = AtomicU64::new(0);
static COUNT_GLOBAL_ALLOCS: AtomicBool = AtomicBool::new(false);
static TRACE_GLOBAL_ALLOCS: AtomicBool = AtomicBool::new(false);

/// Registers the allocation-observer callbacks during process startup.
///
/// Normal and zeroed allocations made after registration feed the same counting
/// and tracing instrumentation.
#[crate::ctor(unsafe)]
fn _static_initialization() {
	// SAFETY: Mutable static globals in jemalloc crate; must be initialized
	// properly and uniquely.
	unsafe { ALLOC = Some(global_alloc_hook) };

	// SAFETY: As above.
	unsafe { ALLOC_ZEROED = Some(global_alloc_zeroed_hook) };
}

/// Returns a human-readable snapshot of allocator memory usage.
///
/// After refreshing the statistics epoch, the report lists allocated, active,
/// mapped, metadata, resident, and retained memory in MiB. It returns `None`
/// when the epoch cannot be refreshed.
#[must_use]
#[cfg(disable)]
//#[cfg(feature = "jemalloc_stats")]
pub fn memory_usage() -> Option<String> {
	use mallctl::stats;

	let mibs = |input: Result<usize, mallctl::Error>| {
		let input = input.unwrap_or_default();
		let kibs = input / 1024;
		let kibs = u32::try_from(kibs).unwrap_or_default();
		let kibs = f64::from(kibs);
		kibs / 1024.0
	};

	// Acquire the epoch; ensure latest stats are pulled in
	acq_epoch().ok()?;

	let allocated = mibs(stats::allocated::read());
	let active = mibs(stats::active::read());
	let mapped = mibs(stats::mapped::read());
	let metadata = mibs(stats::metadata::read());
	let resident = mibs(stats::resident::read());
	let retained = mibs(stats::retained::read());
	Some(format!(
		"allocated: {allocated:.2} MiB\nactive: {active:.2} MiB\nmapped: {mapped:.2} \
		 MiB\nmetadata: {metadata:.2} MiB\nresident: {resident:.2} MiB\nretained: {retained:.2} \
		 MiB\n"
	))
}

/// Returns a human-readable snapshot of allocator memory usage when available.
///
/// Detailed summary reporting is currently disabled for this build, so this
/// implementation returns `None`. Raw allocator output remains available
/// through [`memory_stats`].
#[must_use]
//#[cfg(not(feature = "jemalloc_stats"))]
pub fn memory_usage() -> Option<String> { None }

/// Collects jemalloc's raw statistics report with the supplied print options.
///
/// The allocator statistics epoch is refreshed before invoking
/// `malloc_stats_print`. Refresh failure returns `None`, and output is
/// truncated to 1 MiB.
///
/// # Panics
///
/// Panics if `opts` contains an interior NUL byte.
pub fn memory_stats(opts: &str) -> Option<String> {
	const MAX_LENGTH: usize = 1_048_576;

	let mut str = String::new();
	let opaque = std::ptr::from_mut(&mut str).cast::<c_void>();
	let opts_p: *const c_char = std::ffi::CString::new(opts)
		.expect("cstring")
		.into_raw()
		.cast_const();

	// Acquire the epoch; ensure latest stats are pulled in
	acq_epoch().ok()?;

	// SAFETY: calls malloc_stats_print() with our string instance which must remain
	// in this frame. https://docs.rs/tikv-jemalloc-sys/latest/tikv_jemalloc_sys/fn.malloc_stats_print.html
	unsafe { ffi::malloc_stats_print(Some(malloc_stats_cb), opaque, opts_p) };

	str.truncate(MAX_LENGTH);

	Some(str)
}

/// Appends a jemalloc statistics fragment through the C callback boundary.
///
/// Panics are caught and converted into process aborts before control returns
/// to jemalloc.
///
/// # Safety
///
/// `opaque` must point to the live `String` supplied by [`memory_stats`], and
/// `msg` must point to a NUL-terminated string valid for the duration of the
/// call.
unsafe extern "C" fn malloc_stats_cb(opaque: *mut c_void, msg: *const c_char) {
	catch_unwind(move || handle_malloc_stats(opaque, msg))
		.map_err(|_| abort())
		.ok();
}

fn handle_malloc_stats(opaque: *mut c_void, msg: *const c_char) {
	// SAFETY: we have to trust the opaque points to our String
	let res: &mut String = unsafe {
		opaque
			.cast::<String>()
			.as_mut()
			.expect("failed to cast void* to &mut String")
	};

	// SAFETY: we have to trust the string is null terminated.
	let msg = unsafe { CStr::from_ptr(msg) };

	let msg = String::from_utf8_lossy(msg.to_bytes());
	res.push_str(msg.as_ref());
}

fn global_alloc_hook(layout: Layout) {
	catch_unwind(move || handle_global_alloc(layout))
		.map_err(|_| abort())
		.ok();
}

fn global_alloc_zeroed_hook(layout: Layout) {
	catch_unwind(move || handle_global_alloc(layout))
		.map_err(|_| abort())
		.ok();
}

fn handle_global_alloc(layout: Layout) {
	use std::io::Write;

	use libc::{STDOUT_FILENO, write};

	let do_count = COUNT_GLOBAL_ALLOCS.load(Ordering::Relaxed);
	let count = GLOBAL_ALLOCS.fetch_add(do_count.into(), Ordering::Relaxed);

	if TRACE_GLOBAL_ALLOCS.load(Ordering::Relaxed) {
		let mut buf = ArrayVec::<u8, 128>::new();
		writeln!(&mut buf, "{count} align={} size={}", layout.align(), layout.size())
			.expect("writeln! to buffer failed");

		// SAFETY: Valid ptr and len from buf for writing to stdout.
		unsafe { write(STDOUT_FILENO, buf.as_ptr().cast::<c_void>(), buf.len()) }
			.ge(&0)
			.into_result()
			.expect("write(2) error");
	}
}

/// Returns the process allocation count observed by the allocator hook.
///
/// The counter uses relaxed ordering and advances only when internal allocation
/// counting is enabled. It is intended for allocation measurements rather than
/// synchronized accounting.
#[inline]
#[must_use]
pub fn global_alloc_count() -> u64 { GLOBAL_ALLOCS.load(Ordering::Relaxed) }

macro_rules! mallctl {
	($name:expr_2021) => {{
		thread_local! {
			static KEY: OnceCell<Key> = OnceCell::default();
		};

		KEY.with(|once| {
			once.get_or_init(move || key($name).expect("failed to translate name into mib key"))
				.clone()
		})
	}};
}

/// Controls jemalloc state associated with the calling thread.
///
/// These wrappers expose the thread's arena, cache, profiling state, and
/// allocation counters through mallctl. Control failures are returned through
/// the crate's allocator error type.
pub mod this_thread {
	use super::{Debug, Key, OnceCell, Result, is_nonzero, key, math};

	thread_local! {
		static ALLOCATED_BYTES: OnceCell<&'static u64> = const { OnceCell::new() };
		static DEALLOCATED_BYTES: OnceCell<&'static u64> = const { OnceCell::new() };
	}

	/// Reclaims unused pages from the calling thread's arena.
	///
	/// The operation first applies time-based decay and then purges all
	/// remaining unused dirty pages.
	pub fn trim() -> Result { decay().and_then(|()| purge()) }

	/// Purges unused dirty pages from the calling thread's arena.
	///
	/// This requests immediate reclamation rather than waiting for the arena's
	/// decay schedule.
	pub fn purge() -> Result { notify(mallctl!("arena.0.purge")) }

	/// Applies decay-based purging to the calling thread's arena.
	///
	/// Jemalloc selects unused dirty and muzzy pages according to their
	/// configured decay intervals.
	pub fn decay() -> Result { notify(mallctl!("arena.0.decay")) }

	/// Notifies jemalloc that the calling thread is entering an extended idle
	/// period.
	///
	/// The hint may flush the thread cache and purge its arena, but does not
	/// guarantee a specific cleanup operation.
	pub fn idle() -> Result { super::notify(&mallctl!("thread.idle")) }

	/// Flushes the calling thread's automatic allocation cache.
	///
	/// Cached objects and the cache's internal structures are returned to the
	/// thread's arena.
	pub fn flush() -> Result { super::notify(&mallctl!("thread.tcache.flush")) }

	/// Sets the calling thread's arena muzzy-page decay interval.
	///
	/// A value of `0` requests immediate purging, while `-1` disables purging.
	/// The previous interval is returned.
	pub fn set_muzzy_decay(decay_ms: isize) -> Result<isize> {
		set(mallctl!("arena.0.muzzy_decay_ms"), decay_ms)
	}

	/// Returns the calling thread's arena muzzy-page decay interval.
	///
	/// The value is an approximate delay in milliseconds, with `-1`
	/// representing disabled purging.
	pub fn get_muzzy_decay() -> Result<isize> { get(mallctl!("arena.0.muzzy_decay_ms")) }

	/// Sets the calling thread's arena dirty-page decay interval.
	///
	/// A value of `0` requests immediate purging, while `-1` disables purging.
	/// The previous interval is returned.
	pub fn set_dirty_decay(decay_ms: isize) -> Result<isize> {
		set(mallctl!("arena.0.dirty_decay_ms"), decay_ms)
	}

	/// Returns the calling thread's arena dirty-page decay interval.
	///
	/// The value is an approximate delay in milliseconds, with `-1`
	/// representing disabled purging.
	pub fn get_dirty_decay() -> Result<isize> { get(mallctl!("arena.0.dirty_decay_ms")) }

	/// Enables or disables automatic allocation caching for the calling thread.
	///
	/// Disabling the cache flushes its existing contents. The previous enabled
	/// state is returned.
	pub fn cache_enable(enable: bool) -> Result<bool> {
		super::set::<u8>(&mallctl!("thread.tcache.enabled"), enable.into()).map(is_nonzero!())
	}

	/// Returns whether automatic allocation caching is enabled for the calling
	/// thread.
	///
	/// The value reflects the current thread-specific cache override.
	pub fn is_cache_enabled() -> Result<bool> {
		super::get::<u8>(&mallctl!("thread.tcache.enabled")).map(is_nonzero!())
	}

	/// Associates the calling thread with a jemalloc arena.
	///
	/// Jemalloc initializes an uninitialized target arena as needed. The
	/// previous arena identifier is returned.
	pub fn set_arena(id: usize) -> Result<usize> {
		super::set::<u32>(&mallctl!("thread.arena"), id.try_into()?).and_then(math::try_into)
	}

	/// Returns the jemalloc arena associated with the calling thread.
	///
	/// The allocator's unsigned arena identifier is converted to `usize`.
	pub fn arena_id() -> Result<usize> {
		super::get::<u32>(&mallctl!("thread.arena")).and_then(math::try_into)
	}

	/// Enables or disables allocation sampling for the calling thread.
	///
	/// Global profiling must also be active before this thread produces
	/// samples. The previous thread setting is returned.
	pub fn prof_enable(enable: bool) -> Result<bool> {
		super::set::<u8>(&mallctl!("thread.prof.active"), enable.into()).map(is_nonzero!())
	}

	/// Returns whether allocation sampling is active for the calling thread.
	///
	/// Global profiling can independently suppress sampling even when this
	/// value is true.
	pub fn is_prof_enabled() -> Result<bool> {
		super::get::<u8>(&mallctl!("thread.prof.active")).map(is_nonzero!())
	}

	/// Resets the calling thread's peak net-allocation counter.
	///
	/// Cumulative allocated and deallocated byte counters are not reset.
	pub fn reset_peak() -> Result { super::notify(&mallctl!("thread.peak.reset")) }

	/// Returns the calling thread's approximate peak net allocation in bytes.
	///
	/// The measurement covers time since thread creation or the most recent
	/// peak reset.
	pub fn peak() -> Result<u64> { super::get(&mallctl!("thread.peak.read")) }

	/// Returns the total number of bytes ever allocated by the calling thread.
	///
	/// A cached pointer avoids repeated mallctl lookups, and the underlying
	/// counter can wrap.
	///
	/// # Panics
	///
	/// Panics if jemalloc does not provide a valid thread allocation counter.
	#[inline]
	#[must_use]
	pub fn allocated() -> u64 {
		*ALLOCATED_BYTES.with(|once| init_tls_cell(once, "thread.allocatedp"))
	}

	/// Returns the total number of bytes ever deallocated by the calling
	/// thread.
	///
	/// A cached pointer avoids repeated mallctl lookups, and the underlying
	/// counter can wrap.
	///
	/// # Panics
	///
	/// Panics if jemalloc does not provide a valid thread deallocation counter.
	#[inline]
	#[must_use]
	pub fn deallocated() -> u64 {
		*DEALLOCATED_BYTES.with(|once| init_tls_cell(once, "thread.deallocatedp"))
	}

	fn notify(key: Key) -> Result { super::notify_by_arena(Some(arena_id()?), key) }

	fn set<T>(key: Key, val: T) -> Result<T>
	where
		T: Copy + Debug,
	{
		super::set_by_arena(Some(arena_id()?), key, val)
	}

	fn get<T>(key: Key) -> Result<T>
	where
		T: Copy + Debug,
	{
		super::get_by_arena(Some(arena_id()?), key)
	}

	/// Caches a pointer to one of jemalloc's thread-local byte counters.
	///
	/// The pointer is resolved once per Rust thread and reused by the public
	/// counter accessors.
	///
	/// # Panics
	///
	/// Panics if the mallctl lookup fails or returns a null pointer.
	fn init_tls_cell(cell: &OnceCell<&'static u64>, name: &str) -> &'static u64 {
		cell.get_or_init(|| {
			let ptr: *const u64 = super::get(&mallctl!(name)).expect("failed to obtain pointer");

			// SAFETY: ptr points directly to the internal state of jemalloc for this thread
			unsafe { ptr.as_ref() }.expect("pointer must not be null")
		})
	}
}

/// Resets jemalloc's mutex profiling statistics.
///
/// The reset covers global, arena, and bin mutex counters.
pub fn stats_reset() -> Result { notify(&mallctl!("stats.mutexes.reset")) }

/// Resets accumulated jemalloc heap-profile statistics.
///
/// The control is invoked without specifying a replacement sampling rate.
pub fn prof_reset() -> Result { notify(&mallctl!("prof.reset")) }

/// Writes a jemalloc heap profile to its default dump path.
///
/// Jemalloc derives the filename from the configured profile prefix, process
/// identifier, and dump sequence.
pub fn prof_dump() -> Result { notify(&mallctl!("prof.dump")) }

/// Enables or disables profile dumps at new virtual-memory high-water marks.
///
/// The previous setting is returned. This control is available when jemalloc
/// profiling support is built.
pub fn prof_gdump(enable: bool) -> Result<bool> {
	set::<u8>(&mallctl!("prof.gdump"), enable.into()).map(is_nonzero!())
}

/// Enables or disables global jemalloc allocation sampling.
///
/// Thread-level profiling must also be active before a thread produces samples.
/// The previous global setting is returned.
pub fn prof_enable(enable: bool) -> Result<bool> {
	set::<u8>(&mallctl!("prof.active"), enable.into()).map(is_nonzero!())
}

/// Returns whether global jemalloc allocation sampling is active.
///
/// Thread-level profiling can independently suppress sampling for individual
/// threads.
pub fn is_prof_enabled() -> Result<bool> {
	get::<u8>(&mallctl!("prof.active")).map(is_nonzero!())
}

/// Returns the average allocation interval between periodic heap-profile dumps.
///
/// The interval is measured in allocated bytes and reflects jemalloc's active
/// profiling configuration.
pub fn prof_interval() -> Result<u64> {
	get::<u64>(&mallctl!("prof.interval")).and_then(math::try_into)
}

/// Reclaims unused pages from one arena or from all arenas.
///
/// The operation applies time-based decay before purging remaining unused dirty
/// pages. Passing `None` selects every arena.
pub fn trim<I: Into<Option<usize>> + Copy>(arena: I) -> Result {
	decay(arena).and_then(|()| purge(arena))
}

/// Purges unused dirty pages from one arena or from all arenas.
///
/// Passing `None` selects every arena and requests immediate reclamation.
pub fn purge<I: Into<Option<usize>>>(arena: I) -> Result {
	notify_by_arena(arena.into(), mallctl!("arena.4096.purge"))
}

/// Triggers decay-based purging for one arena or for all arenas.
///
/// Passing `None` selects every arena. Jemalloc chooses unused dirty and muzzy
/// pages according to the configured decay intervals.
pub fn decay<I: Into<Option<usize>>>(arena: I) -> Result {
	notify_by_arena(arena.into(), mallctl!("arena.4096.decay"))
}

/// Sets an arena's muzzy-page decay interval or the default for future arenas.
///
/// Passing `Some` selects an existing arena, while `None` changes the value
/// used to initialize newly created arenas. The previous interval is returned,
/// with `0` requesting immediate purging and `-1` disabling it.
pub fn set_muzzy_decay<I: Into<Option<usize>>>(arena: I, decay_ms: isize) -> Result<isize> {
	match arena.into() {
		| Some(arena) =>
			set_by_arena(Some(arena), mallctl!("arena.4096.muzzy_decay_ms"), decay_ms),
		| _ => set(&mallctl!("arenas.muzzy_decay_ms"), decay_ms),
	}
}

/// Sets an arena's dirty-page decay interval or the default for future arenas.
///
/// Passing `Some` selects an existing arena, while `None` changes the value
/// used to initialize newly created arenas. The previous interval is returned,
/// with `0` requesting immediate purging and `-1` disabling it.
pub fn set_dirty_decay<I: Into<Option<usize>>>(arena: I, decay_ms: isize) -> Result<isize> {
	match arena.into() {
		| Some(arena) =>
			set_by_arena(Some(arena), mallctl!("arena.4096.dirty_decay_ms"), decay_ms),
		| _ => set(&mallctl!("arenas.dirty_decay_ms"), decay_ms),
	}
}

/// Enables or disables jemalloc background purge threads.
///
/// Disabling waits for the workers to terminate before returning. The previous
/// setting is returned.
pub fn background_thread_enable(enable: bool) -> Result<bool> {
	set::<u8>(&mallctl!("background_thread"), enable.into()).map(is_nonzero!())
}

/// Returns whether jemalloc uses a CPU-affine arena mode.
///
/// Query failures produce `false` because this convenience predicate treats an
/// error as no matching affinity mode.
#[inline]
#[must_use]
pub fn is_affine_arena() -> bool { is_percpu_arena() || is_phycpu_arena() }

/// Returns whether jemalloc assigns arenas per logical CPU.
///
/// Query failures produce `false` rather than propagating the control error.
#[inline]
#[must_use]
pub fn is_percpu_arena() -> bool { percpu_arenas().is_ok_and(is_equal_to!("percpu")) }

/// Returns whether jemalloc assigns one arena per physical CPU.
///
/// Sibling hardware threads share an arena in this mode. Query failures produce
/// `false` rather than propagating the control error.
#[inline]
#[must_use]
pub fn is_phycpu_arena() -> bool { percpu_arenas().is_ok_and(is_equal_to!("phycpu")) }

/// Returns jemalloc's configured per-CPU arena mode.
///
/// The static option string is normally `disabled`, `percpu`, or `phycpu`.
pub fn percpu_arenas() -> Result<&'static str> {
	let ptr = get::<*const c_char>(&mallctl!("opt.percpu_arena"))?;
	//SAFETY: ptr points to a null-terminated string returned for opt.percpu_arena.
	let cstr = unsafe { CStr::from_ptr(ptr) };
	cstr.to_str().map_err(Into::into)
}

/// Returns the current limit on automatically managed jemalloc arenas.
///
/// The allocator's unsigned arena count is converted to `usize`.
pub fn arenas() -> Result<usize> {
	get::<u32>(&mallctl!("arenas.narenas")).and_then(math::try_into)
}

/// Refreshes cached allocator statistics by advancing jemalloc's epoch.
///
/// Writing `1` triggers a statistics refresh and returns the resulting epoch
/// value.
pub fn inc_epoch() -> Result<u64> { xchg(&mallctl!("epoch"), 1_u64) }

/// Acquires a fresh snapshot of cached allocator statistics.
///
/// Writing `0` still triggers a statistics refresh, and the resulting epoch
/// value is returned.
pub fn acq_epoch() -> Result<u64> { xchg(&mallctl!("epoch"), 0_u64) }

fn notify_by_arena(id: Option<usize>, mut key: Key) -> Result {
	key[1] = id.unwrap_or(4096);
	notify(&key)
}

fn set_by_arena<T>(id: Option<usize>, mut key: Key, val: T) -> Result<T>
where
	T: Copy + Debug,
{
	key[1] = id.unwrap_or(4096);
	set(&key, val)
}

fn get_by_arena<T>(id: Option<usize>, mut key: Key) -> Result<T>
where
	T: Copy + Debug,
{
	key[1] = id.unwrap_or(4096);
	get(&key)
}

fn notify(key: &Key) -> Result { xchg(key, ()) }

fn set<T>(key: &Key, val: T) -> Result<T>
where
	T: Copy + Debug,
{
	let _lock = CONTROL.lock()?;
	let res = xchg(key, val)?;
	inc_epoch()?;

	Ok(res)
}

#[tracing::instrument(
	name = "get",
	level = "trace"
	skip_all,
	fields(?key)
)]
fn get<T>(key: &Key) -> Result<T>
where
	T: Copy + Debug,
{
	acq_epoch()?;

	// SAFETY: T must be perfectly valid to receive value.
	unsafe { mallctl::raw::read_mib(key.as_slice()) }.map_err(map_err)
}

#[tracing::instrument(
	name = "xchg",
	level = "trace"
	skip_all,
	fields(?key, ?val)
)]
fn xchg<T>(key: &Key, val: T) -> Result<T>
where
	T: Copy + Debug,
{
	// SAFETY: T must be the exact expected type.
	unsafe { mallctl::raw::update_mib(key.as_slice(), val) }.map_err(map_err)
}

fn key(name: &str) -> Result<Key> {
	// tikv asserts the output buffer length is tight to the number of required mibs
	// so we slice that down here.
	let segs = name
		.chars()
		.filter(is_equal_to!(&'.'))
		.count()
		.try_add(1)?;

	let name = self::name(name)?;
	let mut buf = [0_usize; KEY_SEGS];
	mallctl::raw::name_to_mib(name.as_slice(), &mut buf[0..segs])
		.map_err(map_err)
		.map(move |()| buf.into_iter().take(segs).collect())
}

fn name(name: &str) -> Result<Name> {
	let mut buf = Name::new();
	buf.try_extend_from_slice(name.as_bytes())?;
	buf.try_extend_from_slice(b"\0")?;

	Ok(buf)
}

fn map_err(error: jemalloc::ctl::Error) -> crate::Error { err!("mallctl: {}", error.to_string()) }
